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
}
