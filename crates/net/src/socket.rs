//! A WebSocket to the server (ADR 0023): the upgrade is a request like any other, through the stream client (HTTP/1.1 only, no
//! timeout for the whole request, no redirect), with the service's credentials and the gateway's added as for every request (ADR
//! 0017), so the socket uses the same TLS, proxies and plain-HTTP rule as the rest (ADR 0012, ADR 0014). `tokio-tungstenite` frames
//! the upgraded connection; it opens no connection of its own.

use std::time::SystemTime;

use tokio::time::Instant;

use futures_util::{SinkExt, StreamExt};
use reqwest::header::{
    CONNECTION, HeaderMap, HeaderValue, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY,
    SEC_WEBSOCKET_VERSION, UPGRADE,
};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::client::{gateway_for, join};
use crate::log::{LogEntry, Outcome, redact_query};
use crate::{NetError, Request, ServerClient};

/// An open WebSocket. Dropping it closes the connection.
pub struct Socket {
    stream: WebSocketStream<reqwest::Upgraded>,
}

impl std::fmt::Debug for Socket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Socket").finish_non_exhaustive()
    }
}

/// The socket broke: the connection closed or failed, or the server sent what is not a WebSocket frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the connection to the server closed")]
pub struct SocketClosed;

impl Socket {
    /// The next text message; pings are answered and other frames skipped. `Err` once the socket closed.
    ///
    /// # Errors
    /// The connection closed or failed.
    pub async fn next_text(&mut self) -> Result<String, SocketClosed> {
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Text(text))) => return Ok(text.as_str().to_owned()),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return Err(SocketClosed),
                Some(Ok(_)) => {}
            }
        }
    }

    /// Sends a text message.
    ///
    /// # Errors
    /// The connection closed or failed.
    pub async fn send_text(&mut self, text: String) -> Result<(), SocketClosed> {
        self.stream
            .send(Message::text(text))
            .await
            .map_err(|_| SocketClosed)
    }
}

impl ServerClient {
    /// Opens a WebSocket at `request`'s path and query (a GET; its headers are not sent), on the address in use. The answer's
    /// `Set-Cookie` updates the address's gateway cookies, as any answer's does. A failure is logged as a request's is; an open
    /// socket is logged once, with its 101.
    ///
    /// # Errors
    /// - [`NetError::Auth`] for a 401 or 403, [`NetError::Redirected`] for a redirect (never followed: a gateway's sign-in page,
    ///   which the existing gateway flow handles), [`NetError::Status`] for any other answer than 101 or a wrong
    ///   `Sec-WebSocket-Accept`;
    /// - [`NetError::Unreachable`], [`NetError::Timeout`] (no address, or no answer within the request's time) or [`NetError::Tls`];
    /// - [`NetError::InvalidRequest`] for a path that does not fit the address.
    pub async fn open_socket(&self, request: Request) -> Result<Socket, NetError> {
        let started = Instant::now();
        let result = self.upgrade(&request, started).await;
        let outcome = match &result {
            Ok(_) => Outcome::Status(101),
            Err(NetError::Auth { status, .. } | NetError::Status { status, .. }) => {
                Outcome::Status(*status)
            }
            Err(NetError::Redirected { status, .. }) => Outcome::Status(*status),
            Err(error) => Outcome::Error(error.kind()),
        };
        let entry = LogEntry {
            at: SystemTime::now(),
            server: self.server.clone(),
            address: self
                .address
                .current()
                .and_then(|selected| selected.position()),
            method: "GET".to_owned(),
            path: request.path.clone(),
            query: redact_query(&request.query),
            outcome,
            duration: started.elapsed(),
        };
        tracing::debug!(target: "current_net", "{}", entry.to_line());
        self.log.push(entry);
        result
    }

    async fn upgrade(&self, request: &Request, started: Instant) -> Result<Socket, NetError> {
        let http = self.stream_http().await?;
        let deadline = started + self.budget(request);
        let selected = match self.address.current() {
            Some(selected) => Some(selected),
            None => self.address.wait(deadline).await,
        };
        let selected = selected.ok_or(NetError::Unreachable)?;
        let url = join(selected.url(), &request.path, &request.query)?;
        let key = tokio_tungstenite::tungstenite::handshake::client::generate_key();
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
        headers.insert(
            SEC_WEBSOCKET_KEY,
            HeaderValue::from_str(&key).map_err(|_| NetError::InvalidRequest)?,
        );
        let (authorized_url, authorized_headers) = self.authorize(&url, &headers, &selected);
        let send = http.get(authorized_url).headers(authorized_headers).send();
        let remaining = deadline.saturating_duration_since(Instant::now());
        let response = match tokio::time::timeout(remaining, send).await {
            Ok(result) => result.map_err(NetError::from_reqwest)?,
            Err(_) => return Err(NetError::Timeout),
        };
        if let Some(gateway) = gateway_for(&selected, &url) {
            gateway.update(&url, response.headers(), SystemTime::now());
        }
        let status = response.status().as_u16();
        if status != 101 {
            return Err(refusal(status, response, deadline).await);
        }
        let accepted = response
            .headers()
            .get(SEC_WEBSOCKET_ACCEPT)
            .is_some_and(|accept| {
                accept.as_bytes() == derive_accept_key(key.as_bytes()).as_bytes()
            });
        if !accepted {
            return Err(NetError::Status {
                status,
                message: None,
            });
        }
        let upgraded = response.upgrade().await.map_err(NetError::from_reqwest)?;
        Ok(Socket {
            stream: WebSocketStream::from_raw_socket(upgraded, Role::Client, None).await,
        })
    }
}

/// The error for an answer other than 101.
async fn refusal(status: u16, mut response: reqwest::Response, deadline: Instant) -> NetError {
    if (300..400).contains(&status) {
        return NetError::Redirected {
            status,
            location: None,
        };
    }
    let mut body = Vec::new();
    while body.len() < 16 * 1024 {
        match tokio::time::timeout_at(deadline, response.chunk()).await {
            Ok(Ok(Some(chunk))) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    NetError::from_status(status, response.headers(), &body)
}
