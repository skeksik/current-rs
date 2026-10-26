//! Signing in to a Jellyfin server and the session that follows: password, Quick Connect, the current user, sign out (ADR 0015).
//! Behind an authentication gateway (ADR 0017), the header Jellyfin's credentials go in can move to `X-Emby-Authorization`, and a
//! gateway's challenge never ends the session.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use current_model::{ItemId, ItemRef, Permission, ServerId, ServerRef, User, UserId};
use current_net::{
    AddressProvider, ClientConfig, GatewayShape, NetError, Request, RequestLog, Response,
    ServerClient, classify,
};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::auth::{
    self, AccessToken, ClientIdentity, HeaderPlacement, JellyfinAuth, PlacementSlot,
    QuickConnectSecret, TokenSlot,
};
use crate::gateway::{GatewayAnswer, Realm};
use crate::probe::{self, Answer};
use crate::wire;

/// How often Quick Connect asks whether the code was approved: jellyfin-web's interval (5 s).
pub const QUICK_CONNECT_INTERVAL: Duration = Duration::from_secs(5);
/// How long a Quick Connect request lives on the server (`QuickConnectManager.Timeout`, 10 minutes).
pub const QUICK_CONNECT_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// The Quick Connect wait's time: a monotonic clock and a pause. [`RuntimeClock`] is the runtime's; a test passes its own so that
/// it counts checks without waiting for real time.
pub trait WaitClock: Send + Sync {
    /// The time since an arbitrary fixed start.
    fn now(&self) -> Duration;
    /// Waits `duration` on this clock.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// Tokio's clock: what [`JellyfinClient::quick_connect_wait`] uses.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeClock {
    start: Instant,
}

impl RuntimeClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for RuntimeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitClock for RuntimeClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }
}

/// Signing out waits at most this long: the token is deleted on this device whatever the server answers.
const SIGN_OUT_TIMEOUT: Duration = Duration::from_secs(10);
/// The token check after a 401 waits at most this long: the identity probe's request timeout, so that the check's two requests
/// together stay within a default request's deadline.
pub const TOKEN_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// How long [`JellyfinClient::stream_refused`] waits for the check: the identity probe's and the token check's time, and some to
/// spare. Under the player's stall rule (60 s).
pub const STREAM_REFUSAL_WAIT: Duration = Duration::from_secs(30);

/// Why a sign-in or an authenticated request failed. Neither Display nor Debug holds an address, a password or a token.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// 401 to a sign-in: unknown user or wrong password.
    #[error("the user name or password is wrong")]
    WrongCredentials,
    /// 403: the server knows the user but refuses them. Jellyfin answers the same for a disabled or locked-out account, remote
    /// access turned off for the user, a parental time limit, a device the user may not use and the session limit, so they cannot
    /// be told apart.
    #[error("the server refused this user")]
    Refused,
    /// Quick Connect is turned off on the server.
    #[error("Quick Connect is turned off on this server")]
    QuickConnectDisabled,
    /// The Quick Connect request is gone: it was not approved within 10 minutes, or the server restarted.
    #[error("the Quick Connect code expired")]
    QuickConnectExpired,
    /// 401 to a request that carried a token: the token was revoked (signed out elsewhere, replaced by a newer sign-in from this
    /// device, removed by an administrator) or the user was disabled. It can never work again.
    #[error("the session ended: sign in again")]
    SessionRevoked,
    /// The request needs a token and the client has none.
    #[error("not signed in")]
    NotSignedIn,
    /// A 401 without a challenge, which may be Jellyfin's or a gateway's, is being checked with the identity probe
    /// ([`SessionEvent::Unconfirmed`]); requests made while any such check runs get this too. The token is kept meanwhile and
    /// nothing is sent. Try again once the check is done.
    #[error("the server's answer is being checked")]
    Unconfirmed,
    /// 401 to a request whose token still works, as the check after it found ([`JellyfinClient::keep`], ADR 0015 as amended
    /// 2026-10-25): Jellyfin refused this one resource to the user, not the session. Jellyfin 12.2 answers `GET Items` so for a
    /// parent folder the user cannot see (a library hidden from the user, disabled, or removed while the server still knows it).
    /// The session goes on.
    #[error("this is not available to the user")]
    NotAvailable,
    /// An authentication gateway in front of the server answered with a `WWW-Authenticate` challenge, which Jellyfin's own 401 and
    /// 403 never carry (ADR 0017): the request did not reach Jellyfin. Mid-session the token is kept and the session goes on once
    /// the user signs in to the gateway again.
    #[error(transparent)]
    Gateway(GatewayAnswer),
    /// A sign-in sent with `X-Emby-Authorization` answered 400: the server did not get the client's details, so it ignores that
    /// header (ADR 0017). Jellyfin reads it only when legacy authorization is turned on in its settings and `Authorization` arrives
    /// empty (`docs/notes/gateway-auth.md`). The UI explains the two server-side fixes: turn on legacy authorization on the server
    /// and make the gateway stop passing its own `Authorization` header on to the server; or add a gateway rule that lets the
    /// server's API through without the gateway's password.
    #[error(
        "the server ignored the sign-in's client details: turn on legacy authorization on the server and stop the gateway passing \
         its Authorization header on, or let the server's API through the gateway"
    )]
    EmbyHeaderIgnored,
    /// The server could not be reached or answered something unexpected.
    #[error(transparent)]
    Network(#[from] NetError),
}

/// A successful sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub user: User,
    pub token: AccessToken,
    /// The server's id as the sign-in reported it.
    pub server_id: Option<ServerId>,
    /// The libraries the user keeps out of "latest" ([`JellyfinClient::latest_excludes`]), for the client that carries the session
    /// when it is not the one that signed in.
    pub latest_excludes: Vec<ItemRef>,
}

/// A Quick Connect request waiting for approval: the user enters `code` in another signed-in client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickConnectCode {
    pub code: String,
    pub secret: QuickConnectSecret,
}

/// What a signed-in client reports about answers that may not have come from Jellyfin (ADR 0015's 401 rule, as amended by ADR
/// 0017). Debug shows no token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// A 401 without a challenge, a redirect that was not followed or an HTML page where JSON was expected, to a request that
    /// carried `token`: a gateway in front of the server may want a sign-in, or (for a 401 only) Jellyfin ended the session or
    /// refused one resource. The client keeps the token and answers [`AuthError::Unconfirmed`] without sending anything until the
    /// app has re-run the identity probe (and, for Jellyfin's 401, [`JellyfinClient::check_token`]) and called
    /// [`JellyfinClient::revoke`], [`JellyfinClient::keep`] or [`JellyfinClient::resume`]. Reported once per check: such answers
    /// that arrive while one runs join it.
    ///
    /// `unauthorized` is true for a 401. Only then can a passing probe mean the session ended: Jellyfin never answers with a page
    /// or a redirect, and a 403 never ends a session (ADR 0015), so for those a passing probe means the app resumes.
    Unconfirmed {
        token: AccessToken,
        unauthorized: bool,
    },
    /// A gateway's challenge mid-session ([`AuthError::Gateway`]): the token is kept and nothing waits for the app.
    Gateway(GatewayAnswer),
}

/// Called with each [`SessionEvent`].
pub type SessionHook = Arc<dyn Fn(SessionEvent) + Send + Sync>;

/// What the check after a 401 found about the token, as [`JellyfinClient::check_token`] reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenCheck {
    /// Jellyfin answered with the token's user, or refused the user (403) without ending the session: the token works.
    Works,
    /// Jellyfin answered 401 without a challenge: the token is no longer valid.
    Fails,
    /// Anything else: no answer, a gateway's, or the token was replaced meanwhile. Nothing can be decided.
    Unknown,
}

/// How a check of an unconfirmed answer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// The token was forgotten ([`JellyfinClient::revoke`]).
    Revoked,
    /// The token works: the 401 was about the resource ([`JellyfinClient::keep`]).
    Kept,
    /// Nothing was decided ([`JellyfinClient::resume`], a new token).
    Resumed,
    /// The gateway answered the identity probe ([`JellyfinClient::gateway_answered`]): the token is kept.
    Gateway,
}

/// The checks of unconfirmed answers: the one running, if any, and how the last one ended. Checks are numbered from 1.
#[derive(Debug)]
struct Checks {
    /// The token whose answer is being checked.
    running: Option<AccessToken>,
    /// The number of the last check started.
    started: u64,
    /// The number of the last check ended, and how.
    ended: u64,
    outcome: Outcome,
}

/// A Jellyfin client for one server, signed in or not. Every request carries the client's identity; once a token is set, it too.
pub struct JellyfinClient {
    pub(crate) http: ServerClient,
    /// The server's addresses, shared with `http`, for the unauthenticated probe that confirms a gateway refused Jellyfin's header.
    address: Arc<dyn AddressProvider>,
    log: Arc<RequestLog>,
    pub(crate) token: TokenSlot,
    /// The device id the client identifies with: the server keeps one session per device id and user.
    pub(crate) device_id: String,
    placement: PlacementSlot,
    on_event: Mutex<Option<SessionHook>>,
    /// The checks of unconfirmed answers ([`SessionEvent::Unconfirmed`]): requests that got a 401 wait on it for the outcome.
    checks: watch::Sender<Checks>,
    /// The libraries the signed-in user keeps out of "latest" (`Configuration.LatestItemsExcludes`), as the last sign-in or
    /// [`JellyfinClient::current_user`] read them.
    latest_excludes: Mutex<Vec<ItemRef>>,
}

impl std::fmt::Debug for JellyfinClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JellyfinClient")
            .field("server", self.http.server())
            .field("signed_in", &self.has_token())
            .finish_non_exhaustive()
    }
}

impl JellyfinClient {
    /// A client for `server` at `address`, logging to `log`, with the default timeouts.
    pub fn new(
        server: ServerRef,
        address: Arc<dyn AddressProvider>,
        identity: ClientIdentity,
        log: Arc<RequestLog>,
    ) -> Result<Self, NetError> {
        let token = TokenSlot::default();
        let placement = PlacementSlot::default();
        let device_id = identity.device_id.clone();
        let auth = Arc::new(JellyfinAuth {
            identity,
            token: token.clone(),
            placement: placement.clone(),
        });
        let http = ServerClient::new(
            server,
            address.clone(),
            Some(auth),
            log.clone(),
            &ClientConfig::default(),
        )?;
        Ok(Self {
            http,
            address,
            log,
            token,
            device_id,
            placement,
            on_event: Mutex::new(None),
            checks: watch::Sender::new(Checks {
                running: None,
                started: 0,
                ended: 0,
                outcome: Outcome::Resumed,
            }),
            latest_excludes: Mutex::new(Vec::new()),
        })
    }

    pub fn server(&self) -> &ServerRef {
        self.http.server()
    }

    /// Uses `token` for every request from now on. A check of the earlier token's answer no longer holds requests back.
    pub fn set_token(&self, token: AccessToken) {
        auth::write(&self.token, Some(token));
        self.resume();
    }

    /// Forgets the token: requests go out unauthenticated.
    pub fn clear_token(&self) {
        auth::write(&self.token, None);
        self.set_latest_excludes(Vec::new());
        self.resume();
    }

    /// The libraries the signed-in user keeps out of "latest", so Home can skip them (ADR 0018). Read again at each sign-in and
    /// [`JellyfinClient::current_user`], since the user can change it on the server; empty before either and after the session ends.
    pub fn latest_excludes(&self) -> Vec<ItemRef> {
        self.latest_excludes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Sets the libraries the user keeps out of "latest": a sign-in's [`SignedIn::latest_excludes`], when another client made it.
    pub fn set_latest_excludes(&self, libraries: Vec<ItemRef>) {
        // A single assignment under the lock: a poisoned one still holds a usable value.
        *self
            .latest_excludes
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = libraries;
    }

    /// Keeps `user`'s "exclude from latest" libraries.
    fn keep_latest_excludes(&self, user: &wire::UserDto) {
        let libraries = user
            .configuration
            .as_ref()
            .and_then(|configuration| configuration.latest_items_excludes.as_ref())
            .into_iter()
            .flatten()
            .filter(|id| !id.is_empty())
            .map(|id| ItemRef::new(self.server().clone(), ItemId::new(id.as_str())))
            .collect();
        self.set_latest_excludes(libraries);
    }

    pub fn has_token(&self) -> bool {
        auth::read(&self.token).is_some()
    }

    /// Puts Jellyfin's credentials in `placement`'s header from now on: the server's saved setting (ADR 0017), or
    /// [`HeaderPlacement::for_gateway`] for an address with Basic gateway credentials.
    pub fn set_header_placement(&self, placement: HeaderPlacement) {
        auth::set_placement(&self.placement, placement);
    }

    /// The header Jellyfin's credentials go in now. A sign-in can change it (see [`JellyfinClient::sign_in_by_name`]): the app reads
    /// it after one and saves it with the server when it changed.
    pub fn header_placement(&self) -> HeaderPlacement {
        auth::placement(&self.placement)
    }
    /// Sets what runs on each [`SessionEvent`]. Without a hook, nothing can confirm an answer: a 401 without a challenge then
    /// forgets the token at once ([`AuthError::SessionRevoked`]), and a redirect or an HTML page is reported as it is.
    pub fn on_event(&self, hook: SessionHook) {
        *self.on_event.lock().unwrap_or_else(PoisonError::into_inner) = Some(hook);
    }

    /// The identity probe confirmed that the answer to `token` was Jellyfin's 401 (ADR 0015's rule): forgets `token` if it is still
    /// the one in use, so a newer sign-in survives, and ends its check. Returns whether it was forgotten.
    pub fn revoke(&self, token: &AccessToken) -> bool {
        let forgotten = self.forget(token);
        self.end_check(Some(token), Outcome::Revoked);
        forgotten
    }

    /// Ends a check without forgetting the token (the answer was a gateway's, or the probe could not reach the server): requests
    /// go out again, and a request that got the 401 answers [`AuthError::Unconfirmed`].
    pub fn resume(&self) {
        self.end_check(None, Outcome::Resumed);
    }

    /// Ends a check because the identity probe found the gateway's answer (ADR 0017): as [`JellyfinClient::resume`], and a stream
    /// refusal waiting on the check ([`JellyfinClient::stream_refused`]) answers [`AuthError::Gateway`].
    pub fn gateway_answered(&self) {
        self.end_check(None, Outcome::Gateway);
    }

    /// Ends the check of `token`'s 401 once [`JellyfinClient::check_token`] found that the token works (ADR 0015 as amended
    /// 2026-10-25): requests go out again, and a request that got the 401 answers [`AuthError::NotAvailable`]. Nothing happens for a
    /// token no longer checked.
    pub fn keep(&self, token: &AccessToken) {
        self.end_check(Some(token), Outcome::Kept);
    }

    /// Ends the running check (only `token`'s, when given) with `outcome`.
    fn end_check(&self, token: Option<&AccessToken>, outcome: Outcome) {
        self.checks.send_if_modified(|checks| {
            if checks.running.is_none()
                || token.is_some_and(|token| checks.running.as_ref() != Some(token))
            {
                return false;
            }
            checks.running = None;
            checks.ended = checks.started;
            checks.outcome = outcome;
            true
        });
    }

    fn is_checking(&self) -> bool {
        self.checks.borrow().running.is_some()
    }

    /// Asks Jellyfin who the token in use belongs to (`GET /Users/Me`), while a check of `token`'s 401 holds every other request
    /// back: the second part of the check after a 401 the identity probe confirmed as Jellyfin's (ADR 0015 as amended 2026-10-25).
    /// [`TokenCheck::Unknown`] when the client no longer uses `token`.
    pub async fn check_token(&self, token: &AccessToken) -> TokenCheck {
        if auth::read(&self.token).as_ref() != Some(token) {
            return TokenCheck::Unknown;
        }
        let result = self
            .http
            .send(Request::get("/Users/Me").timeout(TOKEN_CHECK_TIMEOUT))
            .await;
        if let Err(error) = &result
            && GatewayAnswer::challenged(error).is_some()
        {
            return TokenCheck::Unknown;
        }
        match result {
            Ok(response) if response.json::<wire::UserDto>().is_ok() => TokenCheck::Works,
            Err(NetError::Auth {
                status: 403,
                page: false,
                ..
            }) => TokenCheck::Works,
            // Jellyfin never answers with a page (ADR 0017): a 401 page is a gateway's, which decides nothing about the token.
            Err(NetError::Auth {
                status: 401,
                page: false,
                ..
            }) => TokenCheck::Fails,
            _ => TokenCheck::Unknown,
        }
    }

    fn hook(&self) -> Option<SessionHook> {
        self.on_event
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Signs in with a user name and password. The password goes into the request body and nowhere else.
    ///
    /// When a gateway answers `Authorization: MediaBrowser ...` with a challenge while an unauthenticated probe of the same address
    /// passes, the gateway refused the header itself: the client moves its credentials to `X-Emby-Authorization` and signs in once
    /// more (ADR 0017). If the server then ignores that header ([`AuthError::EmbyHeaderIgnored`]), the placement goes back.
    pub async fn sign_in_by_name(
        &self,
        username: &str,
        password: &str,
    ) -> Result<SignedIn, AuthError> {
        let body = wire::AuthenticateByName {
            username,
            pw: password,
        };
        let request = Request::post("/Users/AuthenticateByName").json(&body)?;
        let response = self.send_with_fields(request).await.map_err(|error| {
            error.map_net(|error| match error {
                NetError::Auth { status: 401, .. } => AuthError::WrongCredentials,
                other => sign_in_error(other),
            })
        })?;
        self.signed_in(&response)
    }

    /// Whether Quick Connect is turned on.
    pub async fn quick_connect_enabled(&self) -> Result<bool, AuthError> {
        Ok(self
            .http
            .send_json(Request::get("/QuickConnect/Enabled"))
            .await?)
    }

    /// Starts a Quick Connect request. Jellyfin keeps it for 10 minutes. It needs the client's details as a sign-in does, and moves
    /// them to `X-Emby-Authorization` in the same way (see [`JellyfinClient::sign_in_by_name`]).
    pub async fn quick_connect_initiate(&self) -> Result<QuickConnectCode, AuthError> {
        let result: wire::QuickConnectResult = self
            .send_with_fields(Request::post("/QuickConnect/Initiate"))
            .await
            .map_err(|error| error.map_net(quick_connect_error))?
            .json()?;
        Ok(QuickConnectCode {
            code: result.code,
            secret: QuickConnectSecret::new(result.secret),
        })
    }

    /// Whether the request was approved. The secret goes as a query value, which the request log redacts (ADR 0006).
    pub async fn quick_connect_approved(
        &self,
        secret: &QuickConnectSecret,
    ) -> Result<bool, AuthError> {
        let request = Request::get("/QuickConnect/Connect").query("secret", secret.expose());
        let result: wire::QuickConnectResult = self
            .http
            .send_json(request)
            .await
            .map_err(quick_connect_error)?;
        Ok(result.authenticated)
    }

    /// Collects the sign-in of an approved request.
    pub async fn quick_connect_sign_in(
        &self,
        secret: &QuickConnectSecret,
    ) -> Result<SignedIn, AuthError> {
        let body = wire::QuickConnectDto {
            secret: secret.expose(),
        };
        let request = Request::post("/Users/AuthenticateWithQuickConnect").json(&body)?;
        let response = self.http.send(request).await.map_err(quick_connect_error)?;
        self.signed_in(&response)
    }

    /// Asks every `interval` whether the request was approved, then signs in. Ends with [`AuthError::QuickConnectExpired`] once
    /// the server forgot the request or `lifetime` passed. A check that fails on the way (no connection, a timeout, a 5xx while the
    /// server restarts) is retried at the next interval; the last such error is returned if the lifetime runs out on one. Dropping
    /// the future stops the polling.
    pub async fn quick_connect_wait(
        &self,
        secret: &QuickConnectSecret,
        interval: Duration,
        lifetime: Duration,
    ) -> Result<SignedIn, AuthError> {
        self.quick_connect_wait_on(&RuntimeClock::new(), secret, interval, lifetime)
            .await
    }

    /// [`JellyfinClient::quick_connect_wait`] on `clock`: the interval and the lifetime are counted on it.
    pub async fn quick_connect_wait_on(
        &self,
        clock: &impl WaitClock,
        secret: &QuickConnectSecret,
        interval: Duration,
        lifetime: Duration,
    ) -> Result<SignedIn, AuthError> {
        let deadline = clock.now() + lifetime;
        loop {
            let last = match self.quick_connect_approved(secret).await {
                Ok(true) => return self.quick_connect_sign_in(secret).await,
                Ok(false) => None,
                Err(AuthError::Network(error)) if is_transient(&error) => Some(error),
                Err(error) => return Err(error),
            };
            if clock.now() + interval >= deadline {
                return Err(last.map_or(AuthError::QuickConnectExpired, AuthError::Network));
            }
            clock.sleep(interval).await;
        }
    }

    /// The signed-in user (`GET /Users/Me`): checks the token still works.
    pub async fn current_user(&self) -> Result<User, AuthError> {
        let response = self.send_authenticated(Request::get("/Users/Me")).await?;
        let user: wire::UserDto = response.json()?;
        self.keep_latest_excludes(&user);
        Ok(map_user(user, self.server()))
    }

    /// Tells the server to end the session (`POST /Sessions/Logout`), then forgets the token whatever the answer. The caller
    /// deletes the stored token whatever this returns. A 401 here does not run the revoked hook: the session is ending anyway.
    pub async fn sign_out(&self) -> Result<(), AuthError> {
        let Some(token) = auth::read(&self.token) else {
            return Err(AuthError::NotSignedIn);
        };
        let result = self
            .http
            .send(Request::post("/Sessions/Logout").timeout(SIGN_OUT_TIMEOUT))
            .await;
        self.forget(&token);
        match result {
            Ok(_) | Err(NetError::Auth { status: 401, .. }) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Sends a request that needs the token. A 401 or 403 with a `WWW-Authenticate` challenge is a gateway's
    /// ([`AuthError::Gateway`]): the token is kept and the hook hears it (ADR 0017). A 401 without a challenge, a redirect that was
    /// not followed, or an HTML page where JSON was expected may be a gateway's: with a hook, the token is kept and the hook asked
    /// to confirm ([`SessionEvent::Unconfirmed`], the 401 rule's second part). The 401 waits for the app's decision, within the
    /// request's own deadline (its timeout, else the client's), and answers [`AuthError::SessionRevoked`],
    /// [`AuthError::NotAvailable`], or [`AuthError::Unconfirmed`] when nothing was decided in time. A page or a redirect waits for
    /// the check the same way and answers [`AuthError::Gateway`] when the probe found the gateway's answer, else as it would
    /// without a hook ([`AuthError::Refused`] for a 403 page, the network error for a redirect, the page itself for a 2xx one).
    /// Without a hook, a 401 forgets the token (ADR 0015). A 403 never does: the token is valid, only this request was refused.
    pub async fn send_authenticated(&self, request: Request) -> Result<Response, AuthError> {
        self.send_checked(request, true).await
    }

    /// As [`JellyfinClient::send_authenticated`], but a 401 answers [`AuthError::Unconfirmed`] at once instead of waiting for the
    /// app's decision: for requests whose caller must not be held by it (playstate reports, which the player awaits in its loop and
    /// at exit; images, which hold one of the server's fetch slots). The check still starts.
    pub(crate) async fn send_authenticated_now(
        &self,
        request: Request,
    ) -> Result<Response, AuthError> {
        self.send_checked(request, false).await
    }

    async fn send_checked(&self, request: Request, wait: bool) -> Result<Response, AuthError> {
        let Some(token) = auth::read(&self.token) else {
            return Err(AuthError::NotSignedIn);
        };
        if self.is_checking() {
            return Err(AuthError::Unconfirmed);
        }
        let deadline = Instant::now() + self.http.budget(&request);
        let result = self.http.send(request).await;
        // A gateway's challenge, not Jellyfin's answer: the token is kept (ADR 0017, the 401 rule).
        if let Err(error) = &result
            && let Some(answer) = GatewayAnswer::challenged(error)
        {
            if let Some(hook) = self.hook() {
                hook(SessionEvent::Gateway(answer.clone()));
            }
            return Err(AuthError::Gateway(answer));
        }
        let unconfirmed = match &result {
            Err(NetError::Auth { status: 401, .. } | NetError::Redirected { .. }) => true,
            Err(NetError::Auth {
                status: 403, page, ..
            }) => *page,
            Ok(_) => classify(&result) == GatewayShape::SignIn,
            Err(_) => false,
        };
        let unauthorized = matches!(result, Err(NetError::Auth { status: 401, .. }));
        if unconfirmed && let Some(hook) = self.hook() {
            let check = self.begin_check(&token);
            if let Some((_, true)) = check {
                hook(SessionEvent::Unconfirmed {
                    token: token.clone(),
                    unauthorized,
                });
            }
            if unauthorized {
                return Err(match check {
                    Some((number, _)) if wait => match self.check_ended(number, deadline).await {
                        Some(Outcome::Revoked) => AuthError::SessionRevoked,
                        Some(Outcome::Kept) => AuthError::NotAvailable,
                        Some(Outcome::Resumed | Outcome::Gateway) | None => AuthError::Unconfirmed,
                    },
                    _ => AuthError::Unconfirmed,
                });
            }
            // A redirect or a page: once the probe found the gateway's answer, the request says so, so a screen asks for the
            // gateway's sign-in instead of showing its redirect or page (Windows relay check, 2026-10-26).
            if wait
                && let Some((number, _)) = check
                && self.check_ended(number, deadline).await == Some(Outcome::Gateway)
            {
                return Err(AuthError::Gateway(GatewayAnswer::SignIn));
            }
        }
        match result {
            Ok(response) => Ok(response),
            Err(NetError::Auth { status: 401, .. }) => {
                // Without a hook. A newer sign-in may have set another token meanwhile: only the token that failed is forgotten.
                self.forget(&token);
                Err(AuthError::SessionRevoked)
            }
            Err(NetError::Auth { status: 403, .. }) => Err(AuthError::Refused),
            Err(error) => Err(error.into()),
        }
    }

    /// Starts the check of `token`'s answer, or joins the one running. Returns the check's number and whether this started it;
    /// `None` when a newer token replaced `token` (nothing to check).
    fn begin_check(&self, token: &AccessToken) -> Option<(u64, bool)> {
        let mut begun = None;
        self.checks.send_if_modified(|checks| {
            if checks.running.is_some() {
                begun = Some((checks.started, false));
                return false;
            }
            if auth::read(&self.token).as_ref() != Some(token) {
                return false;
            }
            checks.started += 1;
            checks.running = Some(token.clone());
            begun = Some((checks.started, true));
            true
        });
        begun
    }

    /// Waits until check `number` ended, at most until `deadline`, and says how; `None` when it did not end in time. A later check
    /// may have ended meanwhile: its outcome is the newer one and stands for this one too.
    async fn check_ended(&self, number: u64, deadline: Instant) -> Option<Outcome> {
        let mut checks = self.checks.subscribe();
        let ended =
            tokio::time::timeout_at(deadline, checks.wait_for(|checks| checks.ended >= number))
                .await;
        ended.ok().and_then(Result::ok).map(|checks| checks.outcome)
    }

    /// The 401 rule for a refusal of a request this client did not send itself: a stream through the loopback relay, which reports
    /// the answer's status and shape (ADR 0020, "Lifecycle"). It goes through the same check as the client's own answers, so at most
    /// one probe and one token check run per session: a gateway's challenge (Basic or another scheme) is the gateway's at once and
    /// the hook hears it; any other refusal starts the check (the hook confirms it with the identity probe, then, after a 401,
    /// Jellyfin's token check), or joins the one running, and waits for it up to [`STREAM_REFUSAL_WAIT`]. Answers
    /// [`AuthError::Gateway`], [`AuthError::SessionRevoked`], [`AuthError::NotAvailable`] (the token works), or
    /// [`AuthError::Unconfirmed`] when nothing was decided. Without a hook, a 401 forgets the token, as for the client's own.
    pub async fn stream_refused(&self, status: u16, shape: &GatewayShape) -> AuthError {
        let challenged = match shape {
            GatewayShape::Password { realm } => Some(GatewayAnswer::Password {
                realm: realm.as_deref().and_then(Realm::new),
            }),
            GatewayShape::Unsupported => Some(GatewayAnswer::UnsupportedChallenge),
            _ => None,
        };
        if let Some(answer) = challenged {
            if let Some(hook) = self.hook() {
                hook(SessionEvent::Gateway(answer.clone()));
            }
            return AuthError::Gateway(answer);
        }
        let Some(token) = auth::read(&self.token) else {
            return AuthError::NotSignedIn;
        };
        let unauthorized = status == 401;
        let Some(hook) = self.hook() else {
            if unauthorized {
                self.forget(&token);
                return AuthError::SessionRevoked;
            }
            return AuthError::Unconfirmed;
        };
        let Some((number, started)) = self.begin_check(&token) else {
            // A newer sign-in replaced the token: nothing to check.
            return AuthError::Unconfirmed;
        };
        if started {
            hook(SessionEvent::Unconfirmed {
                token,
                unauthorized,
            });
        }
        match self
            .check_ended(number, Instant::now() + STREAM_REFUSAL_WAIT)
            .await
        {
            Some(Outcome::Revoked) => AuthError::SessionRevoked,
            Some(Outcome::Kept) => AuthError::NotAvailable,
            Some(Outcome::Gateway) => AuthError::Gateway(GatewayAnswer::SignIn),
            Some(Outcome::Resumed) | None => AuthError::Unconfirmed,
        }
    }

    /// Sends a request that needs the client's four details (a sign-in, Quick Connect's Initiate). A gateway's challenge to
    /// `Authorization: MediaBrowser ...`, confirmed by an unauthenticated probe of the same address, moves the details to
    /// `X-Emby-Authorization` and sends the request once more: the request provably never reached Jellyfin (ADR 0017). Errors come
    /// back as [`FieldsError`]: a gateway's challenge or the 400 diagnosis, else the server's answer for the caller to map.
    async fn send_with_fields(&self, request: Request) -> Result<Response, FieldsError> {
        let sent = self.header_placement();
        let result = self.http.send(request.clone()).await;
        let (result, sent) = match &result {
            Err(error) => match HeaderPlacement::after_gateway_challenge(sent, error) {
                Some(other) if self.bare_probe_passes().await => {
                    self.set_header_placement(other);
                    let retried = self.http.send(request).await;
                    if is_bad_request(&retried) {
                        // The server ignores the other header: going back keeps a later fix on the gateway's side working.
                        self.set_header_placement(sent);
                    }
                    (retried, other)
                }
                _ => (result, sent),
            },
            Ok(_) => (result, sent),
        };
        result.map_err(|error| {
            if let Some(answer) = GatewayAnswer::challenged(&error) {
                FieldsError::Auth(AuthError::Gateway(answer))
            } else if sent == HeaderPlacement::EmbyAuthorization
                && matches!(error, NetError::Status { status: 400, .. })
            {
                FieldsError::Auth(AuthError::EmbyHeaderIgnored)
            } else {
                FieldsError::Net(error)
            }
        })
    }

    /// Whether the address in use answers the unauthenticated probe as a Jellyfin server: through the same address provider, so
    /// with that address's gateway credentials and never Jellyfin's header or token.
    async fn bare_probe_passes(&self) -> bool {
        let Ok(client) = ServerClient::new(
            self.server().clone(),
            self.address.clone(),
            None,
            self.log.clone(),
            &probe::probe_config(),
        ) else {
            return false;
        };
        matches!(probe::ask(&client).await, Ok(Answer::Found(_)))
    }

    /// Forgets `token` if it is still the one in use. Returns whether it was.
    fn forget(&self, token: &AccessToken) -> bool {
        let mut slot = self.token.write().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref() == Some(token) {
            *slot = None;
            drop(slot);
            self.set_latest_excludes(Vec::new());
            true
        } else {
            false
        }
    }

    fn signed_in(&self, response: &Response) -> Result<SignedIn, AuthError> {
        let result: wire::AuthenticationResult = response.json()?;
        self.keep_latest_excludes(&result.user);
        Ok(SignedIn {
            latest_excludes: self.latest_excludes(),
            user: map_user(result.user, self.server()),
            token: AccessToken::new(result.access_token),
            server_id: result
                .server_id
                .filter(|id| !id.is_empty())
                .map(ServerId::new),
        })
    }
}

/// Why a request that needs the client's details failed: already decided ([`AuthError::Gateway`], [`AuthError::EmbyHeaderIgnored`])
/// or the server's answer, for the caller to map.
enum FieldsError {
    Auth(AuthError),
    Net(NetError),
}

impl FieldsError {
    /// The error, with `map` applied to the server's answer.
    fn map_net(self, map: impl FnOnce(NetError) -> AuthError) -> AuthError {
        match self {
            Self::Auth(error) => error,
            Self::Net(error) => map(error),
        }
    }
}

fn is_bad_request(result: &Result<Response, NetError>) -> bool {
    matches!(result, Err(NetError::Status { status: 400, .. }))
}

/// Errors a later attempt may not meet: the connection, a timeout, or the server failing for a moment.
fn is_transient(error: &NetError) -> bool {
    match error {
        NetError::Unreachable | NetError::Timeout => true,
        NetError::Status { status, .. } => *status >= 500,
        _ => false,
    }
}

fn sign_in_error(error: NetError) -> AuthError {
    if let Some(answer) = GatewayAnswer::challenged(&error) {
        return AuthError::Gateway(answer);
    }
    match error {
        NetError::Auth { status: 403, .. } => AuthError::Refused,
        other => other.into(),
    }
}

/// Quick Connect answers 401 when it is turned off and 404 for a request it no longer knows. A gateway's challenge comes first.
fn quick_connect_error(error: NetError) -> AuthError {
    if let Some(answer) = GatewayAnswer::challenged(&error) {
        return AuthError::Gateway(answer);
    }
    match error {
        NetError::Auth { status: 401, .. } => AuthError::QuickConnectDisabled,
        NetError::Status { status: 404, .. } => AuthError::QuickConnectExpired,
        other => sign_in_error(other),
    }
}

/// Jellyfin's policy flags whose meaning matches one of the model's permissions.
const MAPPED: &[(&str, Permission)] = &[
    ("IsAdministrator", Permission::ADMIN),
    ("EnableContentDownloading", Permission::DOWNLOAD),
    ("EnableContentDeletion", Permission::DELETE),
];
/// Policy flags that are states of the account, not things the user may do.
const NOT_PERMISSIONS: &[&str] = &["IsDisabled", "IsHidden"];

/// The model's user from Jellyfin's. Every policy flag that is `true` becomes a permission: the mapped ones as the model's
/// constants, the rest under Jellyfin's own name (open values, ADR 0005). The avatar is left out: its URL holds the server's
/// address, which the capabilities state must not (ADR 0014).
fn map_user(user: wire::UserDto, server: &ServerRef) -> User {
    let permissions = user
        .policy
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, value)| {
            value.as_bool() == Some(true) && !NOT_PERMISSIONS.contains(&name.as_str())
        })
        .map(|(name, _)| {
            MAPPED
                .iter()
                .find(|(flag, _)| *flag == name)
                .map(|(_, permission)| permission.clone())
                .unwrap_or_else(|| Permission::new(name))
        })
        .collect();
    User {
        id: UserId::new(user.id),
        server: server.clone(),
        name: user.name.unwrap_or_default(),
        permissions,
        avatar: None,
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ServerKey, ServiceType};

    use super::*;

    #[test]
    fn policy_flags_map_to_permissions_and_pass_through() {
        let user: wire::UserDto = serde_json::from_str(
            r#"{"Id":"u1","Name":"Example User","Policy":{"IsAdministrator":true,"IsHidden":true,"IsDisabled":false,
                "EnableContentDownloading":true,"EnableContentDeletion":false,"EnableMediaPlayback":true,
                "SomeFutureFlag":true,"MaxActiveSessions":3,"BlockedTags":[]}}"#,
        )
        .unwrap();
        let server = ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1"));
        let user = map_user(user, &server);
        assert_eq!(user.id, UserId::new("u1"));
        assert_eq!(user.name, "Example User");
        assert!(user.has_permission(&Permission::ADMIN));
        assert!(user.has_permission(&Permission::DOWNLOAD));
        assert!(!user.has_permission(&Permission::DELETE));
        assert!(user.has_permission(&Permission::new("EnableMediaPlayback")));
        assert!(user.has_permission(&Permission::new("SomeFutureFlag")));
        assert!(!user.has_permission(&Permission::new("IsHidden")));
        assert_eq!(user.permissions.len(), 4);
        assert_eq!(user.avatar, None);
    }

    #[test]
    fn a_user_without_a_policy_has_no_permissions() {
        let user: wire::UserDto = serde_json::from_str(r#"{"Id":"u1"}"#).unwrap();
        let user = map_user(
            user,
            &ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        );
        assert!(user.permissions.is_empty());
        assert_eq!(user.name, "");
    }
}
