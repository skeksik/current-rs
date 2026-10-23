//! Capabilities: what each configured server offers right now. Features read this to decide what to show; no feature assumes a
//! service exists.

use crate::identity::{ServerRef, ServiceType};
use crate::open::open_value;
use crate::user::User;

open_value! {
    /// An optional ability a server reports (for example a server setting or a version-dependent endpoint). Open: service crates
    /// define the values they detect; none are known to the model yet.
    Feature {}
}

/// Whether the server answered the last time Current tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Reachability {
    /// Not tried yet.
    #[default]
    Unknown,
    Reachable,
    Unreachable,
}

/// The state of one configured server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerStatus {
    pub server: ServerRef,
    pub signed_in: bool,
    pub reachability: Reachability,
    /// The 1-based place, in the user's list, of the address in use, for the server's settings screen. Never the address itself.
    pub address: Option<u8>,
    /// The signed-in user, once known.
    pub user: Option<User>,
    /// The server's version as it reports it.
    pub version: Option<String>,
    pub features: Vec<Feature>,
}

impl ServerStatus {
    /// A server just configured: not signed in, not tried.
    pub fn new(server: ServerRef) -> Self {
        Self {
            server,
            signed_in: false,
            reachability: Reachability::Unknown,
            address: None,
            user: None,
            version: None,
            features: Vec::new(),
        }
    }

    /// Signed in, so its features exist for the user. It may still be unreachable: its features then show an offline state rather
    /// than disappearing.
    pub fn is_available(&self) -> bool {
        self.signed_in
    }

    pub fn has_feature(&self, feature: &Feature) -> bool {
        self.features.contains(feature)
    }
}

/// One status per configured server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub servers: Vec<ServerStatus>,
}

impl Capabilities {
    pub fn server(&self, server: &ServerRef) -> Option<&ServerStatus> {
        self.servers.iter().find(|status| &status.server == server)
    }

    /// The available servers running `service`.
    pub fn available(&self, service: &ServiceType) -> impl Iterator<Item = &ServerStatus> {
        self.servers
            .iter()
            .filter(move |status| &status.server.service == service && status.is_available())
    }

    /// Whether any server running `service` is available: the question a feature asks before it shows itself.
    pub fn is_available(&self, service: &ServiceType) -> bool {
        self.available(service).next().is_some()
    }

    /// Whether an available server running `service` is also reachable now.
    pub fn is_reachable(&self, service: &ServiceType) -> bool {
        self.available(service)
            .any(|status| status.reachability == Reachability::Reachable)
    }

    /// Whether an available server running `service` reports `feature`.
    pub fn has_feature(&self, service: &ServiceType, feature: &Feature) -> bool {
        self.available(service)
            .any(|status| status.has_feature(feature))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ServerKey;

    fn status(
        service: ServiceType,
        key: &str,
        signed_in: bool,
        reachability: Reachability,
    ) -> ServerStatus {
        ServerStatus {
            signed_in,
            reachability,
            ..ServerStatus::new(ServerRef::new(service, ServerKey::new(key)))
        }
    }

    #[test]
    fn nothing_is_available_without_servers() {
        let capabilities = Capabilities::default();
        assert!(!capabilities.is_available(&ServiceType::JELLYFIN));
        assert!(!capabilities.is_reachable(&ServiceType::JELLYFIN));
    }

    #[test]
    fn a_service_needs_a_signed_in_server() {
        let capabilities = Capabilities {
            servers: vec![
                status(ServiceType::JELLYFIN, "1", false, Reachability::Reachable),
                status(ServiceType::SEERR, "2", true, Reachability::Reachable),
            ],
        };
        assert!(!capabilities.is_available(&ServiceType::JELLYFIN));
        assert!(capabilities.is_available(&ServiceType::SEERR));
        assert!(!capabilities.is_available(&ServiceType::STASH));
    }

    #[test]
    fn an_unreachable_server_stays_available_but_offline() {
        let capabilities = Capabilities {
            servers: vec![status(
                ServiceType::STASH,
                "1",
                true,
                Reachability::Unreachable,
            )],
        };
        assert!(capabilities.is_available(&ServiceType::STASH));
        assert!(!capabilities.is_reachable(&ServiceType::STASH));
    }

    #[test]
    fn features_come_from_available_servers_only() {
        let feature = Feature::new("some_feature");
        let mut signed_out = status(ServiceType::JELLYFIN, "1", false, Reachability::Reachable);
        signed_out.features.push(feature.clone());
        let mut capabilities = Capabilities {
            servers: vec![signed_out],
        };
        assert!(!capabilities.has_feature(&ServiceType::JELLYFIN, &feature));

        capabilities.servers[0].signed_in = true;
        assert!(capabilities.has_feature(&ServiceType::JELLYFIN, &feature));
        assert!(
            capabilities
                .server(&capabilities.servers[0].server.clone())
                .is_some()
        );
    }
}
