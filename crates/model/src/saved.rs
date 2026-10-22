//! What the app remembers about a server it signed in to: kept in the settings by `current-store`, never with a secret.

use crate::identity::{ServerId, ServerRef, UserId};
use crate::server::ServerAddress;

/// A user who signed in to a saved server. The access token is not here: it lives only in the OS keyring (ADR 0013).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedUser {
    pub id: UserId,
    /// The user's name as the server sent it at the last sign-in. Shown, never used as identity.
    pub name: String,
}

/// A server the app keeps between runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedServer {
    /// The app's own key for the server (ADR 0005).
    pub server: ServerRef,
    /// The server's own id, checked on every address before credentials go there (ADR 0012, ADR 0014).
    pub id: ServerId,
    /// The name the server gave itself when it was added. Shown only.
    pub name: String,
    /// Its addresses in the user's order. Never empty for a server that was added through setup.
    pub addresses: Vec<ServerAddress>,
    /// Users who signed in from this install, oldest first.
    pub users: Vec<SavedUser>,
    /// The user whose session the app uses, if one is signed in.
    pub active_user: Option<UserId>,
}

impl SavedServer {
    /// The active user's saved record.
    pub fn active(&self) -> Option<&SavedUser> {
        let active = self.active_user.as_ref()?;
        self.users.iter().find(|user| &user.id == active)
    }
}
