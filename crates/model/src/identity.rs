//! Identity: every object is named by the server it came from plus an id that is unique only within that server.

use crate::open::open_value;

open_value! {
    /// The kind of service a server runs. Open: a service Current does not know yet passes through unchanged.
    ServiceType {
        /// Jellyfin: libraries and playback.
        JELLYFIN = "jellyfin",
        /// Seerr: discover and requests.
        SEERR = "seerr",
        /// Stash: scenes.
        STASH = "stash",
    }
}

/// A key the app assigns to a configured server when it is added. Opaque: never a hostname or URL, so it stays valid when the
/// server's address changes and can appear in logs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerKey(String);

impl ServerKey {
    /// Wraps a key the app assigned.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key as assigned.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ServerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One configured server: the service it runs and the key the app gave it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerRef {
    pub service: ServiceType,
    pub key: ServerKey,
}

impl ServerRef {
    pub fn new(service: ServiceType, key: ServerKey) -> Self {
        Self { service, key }
    }
}

impl std::fmt::Display for ServerRef {
    /// `service/key`, e.g. `jellyfin/1`. Safe for logs: contains no address.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.service, self.key)
    }
}

/// Declares an opaque id: a string from a server, unique only within that server.
macro_rules! opaque_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Wraps an id as the server sent it.
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// The id as the server sent it.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

opaque_id! {
    /// The id of an item (a film, episode, scene, person, ...) on one server. Compare it only together with its server: see
    /// [`ItemRef`].
    ItemId
}

opaque_id! {
    /// The id of a user on one server.
    UserId
}

opaque_id! {
    /// The id a server gives itself, the same on every address it is reached at (a Jellyfin server's `Id`). Tells whether two
    /// addresses lead to the same server; compared byte for byte. Unlike [`ServerKey`], the server chooses it, and a reinstall can
    /// change it.
    ServerId
}

/// An item anywhere: its server and its id there. Two items are the same only if both match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemRef {
    pub server: ServerRef,
    pub id: ItemId,
}

impl ItemRef {
    pub fn new(server: ServerRef, id: ItemId) -> Self {
        Self { server, id }
    }
}

open_value! {
    /// What an item is. Open: each service crate maps its server's kinds to these constants where the meaning matches and passes
    /// any other kind through unchanged.
    MediaKind {
        MOVIE = "movie",
        SERIES = "series",
        SEASON = "season",
        EPISODE = "episode",
        /// A Stash scene.
        SCENE = "scene",
        COLLECTION = "collection",
        PERSON = "person",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(service: ServiceType, key: &str) -> ServerRef {
        ServerRef::new(service, ServerKey::new(key))
    }

    #[test]
    fn the_same_id_on_two_servers_is_two_items() {
        let a = ItemRef::new(server(ServiceType::JELLYFIN, "1"), ItemId::new("42"));
        let b = ItemRef::new(server(ServiceType::JELLYFIN, "2"), ItemId::new("42"));
        let c = ItemRef::new(server(ServiceType::STASH, "1"), ItemId::new("42"));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.clone(), a);
    }

    #[test]
    fn server_ref_displays_without_an_address() {
        assert_eq!(server(ServiceType::SEERR, "home").to_string(), "seerr/home");
    }

    #[test]
    fn unknown_service_passes_through() {
        let other = ServiceType::new("SomeFutureService");
        assert!(!other.is_known());
        assert_eq!(server(other, "x").to_string(), "SomeFutureService/x");
    }
}
