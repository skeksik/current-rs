//! Jellyfin's wire types for the endpoints this crate uses. Private: the public API speaks `current-model`.

use serde::{Deserialize, Serialize};

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

/// `POST /Users/AuthenticateByName` body (`Jellyfin.Api.Models.UserDtos.AuthenticateUserByName`). Borrowed, so the password is
/// never copied into a value of ours.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct AuthenticateByName<'a> {
    pub username: &'a str,
    pub pw: &'a str,
}

/// `POST /Users/AuthenticateWithQuickConnect` body (`QuickConnectDto`).
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct QuickConnectDto<'a> {
    pub secret: &'a str,
}

/// What a sign-in returns (`MediaBrowser.Controller.Authentication.AuthenticationResult`). `SessionInfo` is not read.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct AuthenticationResult {
    pub user: UserDto,
    pub access_token: String,
    pub server_id: Option<String>,
}

/// `MediaBrowser.Model.Dto.UserDto`, the fields this crate reads. `Policy` is read as a map so that flags a newer server adds pass
/// through.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct UserDto {
    pub id: String,
    pub name: Option<String>,
    pub policy: Option<serde_json::Map<String, serde_json::Value>>,
}

/// `MediaBrowser.Model.QuickConnect.QuickConnectResult`, the fields this crate reads.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct QuickConnectResult {
    pub authenticated: bool,
    pub secret: String,
    pub code: String,
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
