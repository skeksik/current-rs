//! Jellyfin's wire types for the endpoints this crate uses. Private: the public API speaks `current-model`.

use serde::Deserialize;

/// `GET /System/Info/Public` (`MediaBrowser.Model.System.PublicSystemInfo`, Jellyfin 12.2). Every field is nullable in the OpenAPI
/// document. `OperatingSystem` is deprecated and never set, so it is not read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PublicSystemInfo {
    pub local_address: Option<String>,
    pub server_name: Option<String>,
    pub version: Option<String>,
    pub product_name: Option<String>,
    pub id: Option<String>,
    pub startup_wizard_completed: Option<bool>,
}

/// A reply to the discovery request (`MediaBrowser.Model.ApiClient.ServerDiscoveryInfo`, Jellyfin 12.2), serialized by
/// System.Text.Json with its default (PascalCase) names. `EndpointAddress` is always null from the server, so it is not read.
/// Unknown fields are ignored, so a newer server's additions do not break discovery.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ServerDiscoveryInfo {
    pub address: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
}
