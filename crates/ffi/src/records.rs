//! Records the bindings carry: flat copies of `current-model` types. Open values (services, features, permissions) cross as their
//! strings, unchanged, so a value no release knows still passes through (ADR 0005).

use current_model as model;

/// What the about and diagnostics screens show about the core.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AppInfo {
    /// The core's version.
    pub version: String,
}

impl From<current_app::AppInfo> for AppInfo {
    fn from(info: current_app::AppInfo) -> Self {
        Self {
            version: info.version,
        }
    }
}

/// What each configured server offers right now. Empty until server setup exists.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct CapabilitiesSnapshot {
    pub servers: Vec<ServerStatusSnapshot>,
}

/// The state of one configured server.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ServerStatusSnapshot {
    /// The service the server runs, e.g. `jellyfin`.
    pub service: String,
    /// The key the app gave the server: never an address.
    pub key: String,
    pub signed_in: bool,
    pub reachability: Reachability,
    /// Which of the server's saved addresses answers, counted from 1 in the user's order (ADR 0014). `None` while none does.
    pub address_position: Option<u8>,
    /// The signed-in user, once known.
    pub user: Option<UserSnapshot>,
    /// The server's version as it reports it.
    pub version: Option<String>,
    pub features: Vec<String>,
}

/// Whether the server answered the last time Current tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Reachability {
    Unknown,
    Reachable,
    Unreachable,
}

/// A user on one server. The avatar is not carried yet: no screen shows it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UserSnapshot {
    pub id: String,
    pub name: String,
    pub permissions: Vec<String>,
}

impl From<&model::Capabilities> for CapabilitiesSnapshot {
    fn from(capabilities: &model::Capabilities) -> Self {
        Self {
            servers: capabilities.servers.iter().map(Into::into).collect(),
        }
    }
}

impl From<&model::ServerStatus> for ServerStatusSnapshot {
    fn from(status: &model::ServerStatus) -> Self {
        Self {
            service: status.server.service.as_str().to_owned(),
            key: status.server.key.as_str().to_owned(),
            signed_in: status.signed_in,
            reachability: status.reachability.into(),
            address_position: status.address,
            user: status.user.as_ref().map(Into::into),
            version: status.version.clone(),
            features: status
                .features
                .iter()
                .map(|feature| feature.as_str().to_owned())
                .collect(),
        }
    }
}

impl From<model::Reachability> for Reachability {
    fn from(reachability: model::Reachability) -> Self {
        match reachability {
            model::Reachability::Unknown => Self::Unknown,
            model::Reachability::Reachable => Self::Reachable,
            model::Reachability::Unreachable => Self::Unreachable,
        }
    }
}

impl From<&model::User> for UserSnapshot {
    fn from(user: &model::User) -> Self {
        Self {
            id: user.id.as_str().to_owned(),
            name: user.name.clone(),
            permissions: user
                .permissions
                .iter()
                .map(|permission| permission.as_str().to_owned())
                .collect(),
        }
    }
}

/// A server as the UI names it in calls: its service and the key the app gave it. Never an address.
#[derive(Clone, Debug, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ServerRef {
    pub service: String,
    pub key: String,
}

impl From<&model::ServerRef> for ServerRef {
    fn from(server: &model::ServerRef) -> Self {
        Self {
            service: server.service.as_str().to_owned(),
            key: server.key.as_str().to_owned(),
        }
    }
}

impl From<&ServerRef> for model::ServerRef {
    fn from(server: &ServerRef) -> Self {
        Self::new(
            model::ServiceType::new(server.service.clone()),
            model::ServerKey::new(server.key.clone()),
        )
    }
}

/// A saved server, for the server list and the account screens. Its addresses stay in the core: only how many there are.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SavedServerSnapshot {
    pub server: ServerRef,
    /// The id the server reports for itself.
    pub id: String,
    pub name: String,
    pub address_count: u32,
    /// The users who signed in on this device, in the order they first did.
    pub users: Vec<SavedUserSnapshot>,
    /// The id of the user signed in now, if any.
    pub active_user: Option<String>,
}

/// A user who signed in on a saved server.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SavedUserSnapshot {
    pub id: String,
    pub name: String,
}

impl From<&model::SavedServer> for SavedServerSnapshot {
    fn from(saved: &model::SavedServer) -> Self {
        Self {
            server: (&saved.server).into(),
            id: saved.id.as_str().to_owned(),
            name: saved.name.clone(),
            address_count: u32::try_from(saved.addresses.len()).unwrap_or(u32::MAX),
            users: saved
                .users
                .iter()
                .map(|user| SavedUserSnapshot {
                    id: user.id.as_str().to_owned(),
                    name: user.name.clone(),
                })
                .collect(),
            active_user: saved.active_user.as_ref().map(|id| id.as_str().to_owned()),
        }
    }
}

/// What the settings store found when the core opened, for the UI to tell the user when it matters.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LoadReport {
    /// First start: new settings were written.
    Created,
    Loaded,
    /// The settings were in an older format, migrated and saved.
    Migrated {
        from: u64,
    },
    /// The settings could not be read: kept as `backup_file` (a file name in the data folder) and reset.
    ResetCorrupt {
        backup_file: String,
    },
    /// The settings come from a newer version of Current: used read-only, and signing in fails until it is updated.
    NewerVersion {
        found: u64,
    },
}

impl From<current_store::LoadReport> for LoadReport {
    fn from(report: current_store::LoadReport) -> Self {
        use current_store::LoadReport as Store;
        match report {
            Store::Created => Self::Created,
            Store::Loaded => Self::Loaded,
            Store::Migrated { from } => Self::Migrated { from },
            Store::ResetCorrupt { backup_file } => Self::ResetCorrupt { backup_file },
            Store::NewerVersion { found } => Self::NewerVersion { found },
        }
    }
}

impl LoadReport {
    /// The report's kind in one word, for the diagnostics header.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Loaded => "loaded",
            Self::Migrated { .. } => "migrated",
            Self::ResetCorrupt { .. } => "reset-corrupt",
            Self::NewerVersion { .. } => "newer-version",
        }
    }
}

/// Where the core keeps secrets (sign-in tokens).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SecretStore {
    /// The OS keyring: Windows Credential Manager, the macOS login keychain. What the apps use.
    Keyring,
    /// Memory only, **for tests**: tokens are lost when the process ends, so every start is signed out.
    Memory,
}

/// The platform's report of the network (ADR 0014), passed in on start and on every change. Debug leaves out the network key.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct NetworkFacts {
    pub connectivity: Connectivity,
    pub metered: Option<bool>,
    pub constrained: Option<bool>,
    pub interface: InterfaceKind,
    pub vpn: Option<bool>,
    /// An opaque key that tells this network from others (for example a hash of the gateway and the Wi-Fi name). It goes into the
    /// core only and is never shown or logged.
    pub network_key: Option<String>,
}

impl std::fmt::Debug for NetworkFacts {
    /// Like a derived Debug, with the network key shown only as present or absent: it can name the Wi-Fi network.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkFacts")
            .field("connectivity", &self.connectivity)
            .field("metered", &self.metered)
            .field("constrained", &self.constrained)
            .field("interface", &self.interface)
            .field("vpn", &self.vpn)
            .field(
                "network_key",
                &self.network_key.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

/// How far the network reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Connectivity {
    /// No network at all.
    Offline,
    LocalOnly,
    CaptivePortal,
    Full,
    Unknown,
}

/// The kind of interface the network uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum InterfaceKind {
    Wifi,
    Ethernet,
    Cellular,
    Other,
    Unknown,
}

impl From<NetworkFacts> for model::NetworkFacts {
    fn from(facts: NetworkFacts) -> Self {
        Self {
            connectivity: match facts.connectivity {
                Connectivity::Offline => model::Connectivity::None,
                Connectivity::LocalOnly => model::Connectivity::LocalOnly,
                Connectivity::CaptivePortal => model::Connectivity::CaptivePortal,
                Connectivity::Full => model::Connectivity::Full,
                Connectivity::Unknown => model::Connectivity::Unknown,
            },
            metered: facts.metered,
            constrained: facts.constrained,
            interface: match facts.interface {
                InterfaceKind::Wifi => model::InterfaceKind::WiFi,
                InterfaceKind::Ethernet => model::InterfaceKind::Ethernet,
                InterfaceKind::Cellular => model::InterfaceKind::Cellular,
                InterfaceKind::Other => model::InterfaceKind::Other,
                InterfaceKind::Unknown => model::InterfaceKind::Unknown,
            },
            vpn: facts.vpn,
            network_key: facts.network_key.map(model::NetworkKey::new),
        }
    }
}

/// How a sign-out went. The app is signed out either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SignOutResult {
    /// Whether the server heard of it. When false, the session may stay listed on the server until it expires; the UI may say so.
    pub server_told: bool,
}

/// How removing a saved server went. The server is told in the background; its answer is not waited for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RemoveServerResult {
    /// False when no such server was saved (unknown, or removed already): nothing changed.
    pub removed: bool,
    /// A saved sign-in could not be deleted from the keyring (Credential Manager, the keychain); the UI says so.
    pub credentials_left: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{
        Feature, Permission, ServerKey, ServerRef, ServerStatus, ServiceType, User, UserId,
    };

    #[test]
    fn open_values_pass_through_unchanged() {
        let server = ServerRef::new(ServiceType::new("SomeFutureService"), ServerKey::new("7"));
        let status = ServerStatus {
            signed_in: true,
            reachability: model::Reachability::Unreachable,
            user: Some(User {
                id: UserId::new("u1"),
                server: server.clone(),
                name: "Example".into(),
                permissions: vec![
                    Permission::REQUEST,
                    Permission::new("SOME_FUTURE_PERMISSION"),
                ],
                avatar: None,
            }),
            version: Some("10.11.0".into()),
            features: vec![Feature::new("Some_Feature")],
            ..ServerStatus::new(server)
        };
        let snapshot = CapabilitiesSnapshot::from(&model::Capabilities {
            servers: vec![status],
        });
        assert_eq!(
            snapshot.servers,
            vec![ServerStatusSnapshot {
                service: "SomeFutureService".into(),
                key: "7".into(),
                signed_in: true,
                reachability: Reachability::Unreachable,
                address_position: None,
                user: Some(UserSnapshot {
                    id: "u1".into(),
                    name: "Example".into(),
                    permissions: vec!["request".into(), "SOME_FUTURE_PERMISSION".into()],
                }),
                version: Some("10.11.0".into()),
                features: vec!["Some_Feature".into()],
            }]
        );
    }

    #[test]
    fn debug_hides_the_network_key() {
        let facts = NetworkFacts {
            connectivity: Connectivity::Full,
            metered: None,
            constrained: None,
            interface: InterfaceKind::Wifi,
            vpn: None,
            network_key: Some("home-wifi-192.168.1.10".into()),
        };
        let debug = format!("{facts:?}");
        assert!(debug.contains("[redacted]"), "{debug}");
        assert!(!debug.contains("home-wifi"), "{debug}");
        assert!(!debug.contains("192.168"), "{debug}");
    }
}
