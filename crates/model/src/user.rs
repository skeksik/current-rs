//! Users: who is signed in to a server and what they may do there.

use crate::artwork::Artwork;
use crate::identity::{ServerRef, UserId};
use crate::open::open_value;

open_value! {
    /// Something a user may do on a server. Open: each service crate maps its server's permissions to these where the meaning
    /// matches and passes any other permission through unchanged.
    Permission {
        /// Administers the server.
        ADMIN = "admin",
        /// May request new content.
        REQUEST = "request",
        /// May approve or decline other users' requests.
        MANAGE_REQUESTS = "manage_requests",
        /// May download media.
        DOWNLOAD = "download",
        /// May delete media.
        DELETE = "delete",
    }
}

/// A user on one server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub server: ServerRef,
    pub name: String,
    pub permissions: Vec<Permission>,
    pub avatar: Option<Artwork>,
}

impl User {
    pub fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions.contains(permission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ServerKey, ServiceType};

    #[test]
    fn permissions_include_unknown_values() {
        let user = User {
            id: UserId::new("u1"),
            server: ServerRef::new(ServiceType::SEERR, ServerKey::new("1")),
            name: "Example".into(),
            permissions: vec![
                Permission::REQUEST,
                Permission::new("SOME_FUTURE_PERMISSION"),
            ],
            avatar: None,
        };
        assert!(user.has_permission(&Permission::REQUEST));
        assert!(user.has_permission(&Permission::new("SOME_FUTURE_PERMISSION")));
        assert!(!user.has_permission(&Permission::ADMIN));
    }
}
