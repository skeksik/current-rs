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
