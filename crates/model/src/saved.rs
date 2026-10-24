//! What the app remembers about a server it signed in to: kept in the settings by `current-store`, never with a secret.

use crate::identity::{ServerId, ServerRef, UserId};
use crate::open::open_value;
use crate::server::{AddressId, ServerAddress};

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
    /// The highest address id ever given out on this server, so a removed address's id is never given to another one.
    pub last_address_id: u64,
    /// Which header carries the service's own credentials (Jellyfin's, ADR 0015 as amended by ADR 0017). An unknown value is used
    /// as [`AuthHeader::AUTHORIZATION`] and kept as it is.
    pub header_placement: AuthHeader,
}

open_value! {
    /// The header a service's own credentials go in (ADR 0017, "Jellyfin's header placement"). Open: a value a newer version
    /// stored passes through unchanged.
    AuthHeader {
        /// `Authorization`, the default.
        AUTHORIZATION = "authorization",
        /// `X-Emby-Authorization`, leaving `Authorization` to a gateway.
        X_EMBY_AUTHORIZATION = "x-emby-authorization",
    }
}

impl Default for AuthHeader {
    fn default() -> Self {
        Self::AUTHORIZATION
    }
}

impl SavedServer {
    /// The active user's saved record.
    pub fn active(&self) -> Option<&SavedUser> {
        let active = self.active_user.as_ref()?;
        self.users.iter().find(|user| &user.id == active)
    }

    /// Adds the address at the end with a new id, and returns the id.
    pub fn add_address(&mut self, mut address: ServerAddress) -> AddressId {
        let id = self.next_address_id();
        address.id = Some(id);
        self.addresses.push(address);
        id
    }

    /// Gives every address without an id, or with an id an earlier address already has, a new one. For addresses that were not
    /// added through [`add_address`](Self::add_address): older settings files, or a hand-edited one. Returns whether any changed.
    pub fn assign_address_ids(&mut self) -> bool {
        // Ids already in use count as given out, whatever `last_address_id` says.
        let highest = self
            .addresses
            .iter()
            .filter_map(|a| a.id)
            .map(AddressId::get)
            .max();
        self.last_address_id = self.last_address_id.max(highest.unwrap_or(0));
        let mut seen = Vec::with_capacity(self.addresses.len());
        let mut changed = false;
        for index in 0..self.addresses.len() {
            let id = match self.addresses[index].id {
                Some(id) if !seen.contains(&id) => id,
                _ => {
                    changed = true;
                    let id = self.next_address_id();
                    self.addresses[index].id = Some(id);
                    id
                }
            };
            seen.push(id);
        }
        changed
    }

    fn next_address_id(&mut self) -> AddressId {
        self.last_address_id += 1;
        AddressId::new(self.last_address_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ServerKey, ServiceType};

    fn server() -> SavedServer {
        SavedServer {
            server: ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            id: ServerId::new("x"),
            name: String::new(),
            addresses: Vec::new(),
            users: Vec::new(),
            active_user: None,
            last_address_id: 0,
            header_placement: AuthHeader::default(),
        }
    }

    fn ids(saved: &SavedServer) -> Vec<Option<u64>> {
        saved
            .addresses
            .iter()
            .map(|a| a.id.map(AddressId::get))
            .collect()
    }

    #[test]
    fn added_addresses_get_ids_that_are_never_given_again() {
        let mut saved = server();
        assert_eq!(
            saved.add_address(ServerAddress::new("https://media.example.com", false)),
            AddressId::new(1)
        );
        saved.add_address(ServerAddress::new("http://192.168.1.10:8096", true));
        saved.addresses.remove(1);
        saved.add_address(ServerAddress::new("http://127.0.0.1:8096", true));
        assert_eq!(ids(&saved), [Some(1), Some(3)]);
        assert_eq!(saved.last_address_id, 3);
    }

    #[test]
    fn missing_and_duplicate_ids_are_replaced_and_others_kept() {
        let mut saved = server();
        let with = |id: Option<u64>| {
            let mut address = ServerAddress::new("https://media.example.com", false);
            address.id = id.map(AddressId::new);
            address
        };
        saved.addresses = vec![with(Some(4)), with(None), with(Some(4)), with(Some(2))];
        assert!(saved.assign_address_ids());
        assert_eq!(ids(&saved), [Some(4), Some(5), Some(6), Some(2)]);
        assert!(!saved.assign_address_ids());
        // Reordering keeps every id.
        saved.addresses.reverse();
        assert!(!saved.assign_address_ids());
        assert_eq!(ids(&saved), [Some(2), Some(6), Some(5), Some(4)]);
    }
}
