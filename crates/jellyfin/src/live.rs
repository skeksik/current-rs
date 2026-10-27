//! Live updates from Jellyfin's WebSocket (ADR 0023): the user data another client, playback or the server changed, as they change.
//!
//! Jellyfin 12.3 (tag 06de749, read 2026-10-27):
//! - opens a socket on any path that asks for an upgrade (`WebSocketHandlerMiddleware.Invoke`), for the session its `Authorization`
//!   names (`AuthorizationContext`, `RequestHelpers.GetSession`); no message subscribes to anything (`SessionWebSocketListener`
//!   registers the socket as the session's controller on connect);
//! - frames every message as `{ "MessageType", "Data", "MessageId" }` (`OutboundWebSocketMessage`);
//! - sends `ForceKeepAlive` with the seconds it waits for a `KeepAlive` before it drops the socket (`WebSocketLostTimeout = 60`), and
//!   counts only a `KeepAlive` message as one, not a ping (`WebSocketConnection`: `LastKeepAliveDate`);
//! - sends `UserDataChanged` with `UserDataChangeInfo { UserId, UserDataList }`, every save of the user's data but a progress
//!   report's, with each item's parent, in batches of 500 ms (`UserDataChangeNotifier`).
//!
//! The socket does not make the session one that others can control: `SessionInfo.SupportsMediaControl` is false unless the
//! client's capabilities say "`SupportsMediaControl`" ("`if (Capabilities is null || !Capabilities.SupportsMediaControl) return
//! false;`"), which Current never posts, and a new session's capabilities are the device's saved ones or none
//! (`DeviceManager.GetCapabilities`: "`: new()`", whose `SupportsMediaControl` is false). So remote control commands (`Play`,
//! `GeneralCommand`, `Playstate`) do not come, and are ignored if they do.

use std::time::Duration;

use current_model::{ItemId, ItemRef, ServerRef, UserState};
use current_net::{NetError, Request, Socket, SocketClosed};
use serde::Deserialize;

use crate::browse_wire::UserItemDataDto;
use crate::gateway::GatewayAnswer;
use crate::{AuthError, JellyfinClient, auth};

/// What the server sent on the socket.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveMessage {
    /// The user data of these items changed.
    UserData(Vec<(ItemRef, UserState)>),
    /// The server drops the socket after this long without a [`LiveSocket::keep_alive`].
    ForceKeepAlive(Duration),
    /// Anything else (remote control, sessions, library changes, kinds not known yet): ignored.
    Other,
}

/// The session's open socket. Dropping it closes it.
#[derive(Debug)]
pub struct LiveSocket {
    socket: Socket,
    server: ServerRef,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Envelope {
    message_type: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserDataChangeInfo {
    #[serde(default)]
    user_data_list: Vec<UserItemDataDto>,
}

impl JellyfinClient {
    /// Opens the session's socket (`/socket`), with the session's token and the gateway's credentials as any request.
    ///
    /// # Errors
    /// [`AuthError::NotSignedIn`] without a token; [`AuthError::Gateway`] for a gateway's challenge or sign-in page;
    /// [`AuthError::Refused`] for Jellyfin's refusal (403 for a token it does not take); else the network's failure. Nothing here
    /// forgets the token or starts a check: the session's requests find out on their own.
    pub async fn live(&self) -> Result<LiveSocket, AuthError> {
        if auth::read(&self.token).is_none() {
            return Err(AuthError::NotSignedIn);
        }
        match self.http.open_socket(Request::get("/socket")).await {
            Ok(socket) => Ok(LiveSocket {
                socket,
                server: self.server().clone(),
            }),
            Err(error) => Err(match GatewayAnswer::challenged(&error) {
                Some(answer) => AuthError::Gateway(answer),
                None => match error {
                    NetError::Redirected { .. } => AuthError::Gateway(GatewayAnswer::SignIn),
                    NetError::Auth { .. } => AuthError::Refused,
                    error => error.into(),
                },
            }),
        }
    }
}

impl LiveSocket {
    /// The next message.
    ///
    /// # Errors
    /// The socket closed.
    pub async fn next(&mut self) -> Result<LiveMessage, SocketClosed> {
        let text = self.socket.next_text().await?;
        Ok(self.message(&text))
    }

    /// Tells the server the client is still there.
    ///
    /// # Errors
    /// The socket closed.
    pub async fn keep_alive(&mut self) -> Result<(), SocketClosed> {
        self.socket
            .send_text(r#"{"MessageType":"KeepAlive"}"#.to_owned())
            .await
    }

    fn message(&self, text: &str) -> LiveMessage {
        parse(text, |id| {
            ItemRef::new(self.server.clone(), ItemId::new(id))
        })
    }
}

/// A message's text as [`LiveMessage`]; `item` names an item by its id.
fn parse(text: &str, item: impl Fn(&str) -> ItemRef) -> LiveMessage {
    let Ok(envelope) = serde_json::from_str::<Envelope>(text) else {
        return LiveMessage::Other;
    };
    match envelope.message_type.as_str() {
        "ForceKeepAlive" => envelope
            .data
            .as_u64()
            .filter(|seconds| *seconds > 0)
            .map_or(LiveMessage::Other, |seconds| {
                LiveMessage::ForceKeepAlive(Duration::from_secs(seconds))
            }),
        "UserDataChanged" => {
            let Ok(info) = serde_json::from_value::<UserDataChangeInfo>(envelope.data) else {
                return LiveMessage::Other;
            };
            LiveMessage::UserData(
                info.user_data_list
                    .into_iter()
                    .filter_map(|data| {
                        let id = data.item_id.clone().filter(|id| !id.is_empty())?;
                        Some((item(&id), crate::browse::user_state(Some(data))))
                    })
                    .collect(),
            )
        }
        _ => LiveMessage::Other,
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ServerKey, ServiceType};

    use super::*;

    fn item(id: &str) -> ItemRef {
        ItemRef::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            ItemId::new(id),
        )
    }

    #[test]
    fn user_data_and_keep_alive_are_read_and_the_rest_ignored() {
        assert_eq!(
            parse(
                r#"{"MessageType":"ForceKeepAlive","Data":60,"MessageId":"57a2d1e0000000000000000000000030"}"#,
                item
            ),
            LiveMessage::ForceKeepAlive(Duration::from_secs(60))
        );
        // As `UserDataChangeNotifier` sends it: `UserDataChangeInfo` with each item's `UserItemDataDto`, its `ItemId` included.
        let changed = parse(
            r#"{"MessageType":"UserDataChanged","Data":{"UserId":"57a2d1e0000000000000000000000002","UserDataList":[
                {"PlaybackPositionTicks":0,"PlayCount":1,"IsFavorite":true,"Played":true,"Key":"57a2d1e0000000000000000000002001","ItemId":"57a2d1e0000000000000000000002001"},
                {"PlaybackPositionTicks":0,"PlayedPercentage":50,"UnplayedItemCount":1,"PlayCount":0,"IsFavorite":false,"Played":false,"Key":"x"}
            ]},"MessageId":"57a2d1e0000000000000000000000031"}"#,
            item,
        );
        let LiveMessage::UserData(changed) = changed else {
            panic!("{changed:?}");
        };
        // The entry without an item id is left out.
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, item("57a2d1e0000000000000000000002001"));
        assert!(changed[0].1.played && changed[0].1.favourite);
        for other in [
            r#"{"MessageType":"Play","Data":{"ItemIds":["57a2d1e0000000000000000000002001"],"PlayCommand":"PlayNow"}}"#,
            r#"{"MessageType":"GeneralCommand","Data":{"Name":"DisplayMessage"}}"#,
            r#"{"MessageType":"Playstate","Data":{"Command":"Stop"}}"#,
            r#"{"MessageType":"SomethingNew"}"#,
            r#"{"MessageType":"ForceKeepAlive","Data":0}"#,
            "not json",
        ] {
            assert_eq!(parse(other, item), LiveMessage::Other, "{other}");
        }
    }
}
