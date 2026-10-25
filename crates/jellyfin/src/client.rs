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
use tokio::time::Instant;

use crate::auth::{
    self, AccessToken, ClientIdentity, HeaderPlacement, JellyfinAuth, PlacementSlot,
    QuickConnectSecret, TokenSlot,
};
use crate::gateway::GatewayAnswer;
use crate::probe::{self, Answer};
use crate::wire;

/// How often Quick Connect asks whether the code was approved: jellyfin-web's interval (5 s).
pub const QUICK_CONNECT_INTERVAL: Duration = Duration::from_secs(5);
/// How long a Quick Connect request lives on the server (`QuickConnectManager.Timeout`, 10 minutes).
pub const QUICK_CONNECT_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// Signing out waits at most this long: the token is deleted on this device whatever the server answers.
const SIGN_OUT_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// carried `token`: a gateway in front of the server may want a sign-in, or (for a 401 only) Jellyfin ended the session. The
    /// client keeps the token and answers [`AuthError::Unconfirmed`] without sending anything until the app has re-run the
    /// identity probe and called [`JellyfinClient::revoke`] or [`JellyfinClient::resume`]. Reported once per check: such answers
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
    /// The token whose unconfirmed answer the app is checking, if a check runs ([`SessionEvent::Unconfirmed`]).
    checking: Mutex<Option<AccessToken>>,
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
            checking: Mutex::new(None),
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
        let mut checking = self.lock_checking();
        if checking.as_ref() == Some(token) {
            *checking = None;
        }
        forgotten
    }

    /// Ends a check without forgetting the token (the answer was a gateway's, or the probe could not reach the server): requests
    /// go out again.
    pub fn resume(&self) {
        *self.lock_checking() = None;
    }

    fn lock_checking(&self) -> std::sync::MutexGuard<'_, Option<AccessToken>> {
        // A single assignment under the lock: a poisoned one still holds a usable value.
        self.checking.lock().unwrap_or_else(PoisonError::into_inner)
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
        let deadline = Instant::now() + lifetime;
        loop {
            let last = match self.quick_connect_approved(secret).await {
                Ok(true) => return self.quick_connect_sign_in(secret).await,
                Ok(false) => None,
                Err(AuthError::Network(error)) if is_transient(&error) => Some(error),
                Err(error) => return Err(error),
            };
            if Instant::now() + interval >= deadline {
                return Err(last.map_or(AuthError::QuickConnectExpired, AuthError::Network));
            }
            tokio::time::sleep(interval).await;
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
    /// to confirm ([`SessionEvent::Unconfirmed`], the 401 rule's second part). The 401 answers [`AuthError::Unconfirmed`]; a page
    /// or a redirect answers as it would without a hook ([`AuthError::Refused`] for a 403 page, the network error for a redirect,
    /// the page itself for a 2xx one). Without a hook, a 401 forgets the token (ADR 0015). A 403 never does: the token is valid,
    /// only this request was refused.
    pub async fn send_authenticated(&self, request: Request) -> Result<Response, AuthError> {
        let Some(token) = auth::read(&self.token) else {
            return Err(AuthError::NotSignedIn);
        };
        if self.lock_checking().is_some() {
            return Err(AuthError::Unconfirmed);
        }
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
            if self.begin_check(&token) {
                hook(SessionEvent::Unconfirmed {
                    token: token.clone(),
                    unauthorized,
                });
            }
            if unauthorized {
                return Err(AuthError::Unconfirmed);
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

    /// Starts the check of `token`'s answer, unless one runs already or a newer token replaced it. Returns whether it started.
    fn begin_check(&self, token: &AccessToken) -> bool {
        let mut checking = self.lock_checking();
        if checking.is_some() || auth::read(&self.token).as_ref() != Some(token) {
            return false;
        }
        *checking = Some(token.clone());
        true
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
