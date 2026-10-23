//! Signing in to a Jellyfin server and the session that follows: password, Quick Connect, the current user, sign out (ADR 0015).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use current_model::{Permission, ServerId, ServerRef, User, UserId};
use current_net::{
    AddressProvider, ClientConfig, NetError, Request, RequestLog, Response, ServerClient,
};
use tokio::time::Instant;

use crate::auth::{self, AccessToken, ClientIdentity, JellyfinAuth, QuickConnectSecret, TokenSlot};
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
}

/// A Quick Connect request waiting for approval: the user enters `code` in another signed-in client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickConnectCode {
    pub code: String,
    pub secret: QuickConnectSecret,
}

/// Called when a request that carried a token gets 401.
pub type RevokedHook = Arc<dyn Fn() + Send + Sync>;

/// A Jellyfin client for one server, signed in or not. Every request carries the client's identity; once a token is set, it too.
pub struct JellyfinClient {
    http: ServerClient,
    token: TokenSlot,
    on_revoked: Mutex<Option<RevokedHook>>,
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
        let auth = Arc::new(JellyfinAuth {
            identity,
            token: token.clone(),
        });
        let http = ServerClient::new(server, address, Some(auth), log, &ClientConfig::default())?;
        Ok(Self {
            http,
            token,
            on_revoked: Mutex::new(None),
        })
    }

    pub fn server(&self) -> &ServerRef {
        self.http.server()
    }

    /// Uses `token` for every request from now on.
    pub fn set_token(&self, token: AccessToken) {
        auth::write(&self.token, Some(token));
    }

    /// Forgets the token: requests go out unauthenticated.
    pub fn clear_token(&self) {
        auth::write(&self.token, None);
    }

    pub fn has_token(&self) -> bool {
        auth::read(&self.token).is_some()
    }

    /// Sets what runs when a request that carried a token gets 401. The client has already forgotten the token by then.
    pub fn on_revoked(&self, hook: RevokedHook) {
        *self
            .on_revoked
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(hook);
    }

    /// Signs in with a user name and password. The password goes into the request body and nowhere else.
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
        let response = self.http.send(request).await.map_err(|error| match error {
            NetError::Auth { status: 401, .. } => AuthError::WrongCredentials,
            other => sign_in_error(other),
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

    /// Starts a Quick Connect request. Jellyfin keeps it for 10 minutes.
    pub async fn quick_connect_initiate(&self) -> Result<QuickConnectCode, AuthError> {
        let result: wire::QuickConnectResult = self
            .http
            .send_json(Request::post("/QuickConnect/Initiate"))
            .await
            .map_err(quick_connect_error)?;
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

    /// Sends a request that needs the token. A 401 means the token is dead: the client forgets it and runs the revoked hook
    /// (ADR 0015). A 403 does not: the token is valid, only this request was refused.
    pub async fn send_authenticated(&self, request: Request) -> Result<Response, AuthError> {
        let Some(token) = auth::read(&self.token) else {
            return Err(AuthError::NotSignedIn);
        };
        match self.http.send(request).await {
            Ok(response) => Ok(response),
            Err(NetError::Auth { status: 401, .. }) => {
                // A newer sign-in may have set another token meanwhile: only the token that failed is forgotten.
                if self.forget(&token) {
                    let hook = self
                        .on_revoked
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone();
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                Err(AuthError::SessionRevoked)
            }
            Err(NetError::Auth { status: 403, .. }) => Err(AuthError::Refused),
            Err(error) => Err(error.into()),
        }
    }

    /// Forgets `token` if it is still the one in use. Returns whether it was.
    fn forget(&self, token: &AccessToken) -> bool {
        let mut slot = self.token.write().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref() == Some(token) {
            *slot = None;
            true
        } else {
            false
        }
    }

    fn signed_in(&self, response: &Response) -> Result<SignedIn, AuthError> {
        let result: wire::AuthenticationResult = response.json()?;
        Ok(SignedIn {
            user: map_user(result.user, self.server()),
            token: AccessToken::new(result.access_token),
            server_id: result
                .server_id
                .filter(|id| !id.is_empty())
                .map(ServerId::new),
        })
    }
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
    match error {
        NetError::Auth { status: 403, .. } => AuthError::Refused,
        other => other.into(),
    }
}

/// Quick Connect answers 401 when it is turned off and 404 for a request it no longer knows.
fn quick_connect_error(error: NetError) -> AuthError {
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
