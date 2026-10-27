//! The HTTP client for one server: its address comes from an [`AddressProvider`], every request has a timeout, errors are typed by
//! what the UI does about them, and every request goes into a redacted, bounded [`RequestLog`] (ADR 0006).
//!
//! Service crates (Jellyfin, Seerr, Stash) build on [`ServerClient`] and add their credentials through [`RequestAuth`]. [`transport`]
//! tells whether plain HTTP to an address needs the user's confirmation (ADR 0012). An authentication gateway in front of an address
//! gets its credentials through a [`Gateway`], and [`classify`] tells a gateway's answer from the service's (ADR 0017).
//! [`ServerClient::send_stream`] streams an answer chunk by chunk for the loopback relay (ADR 0020).

mod address;
mod challenge;
mod client;
mod error;
mod gateway;
mod log;
mod roaming;
mod scope;
mod shape;
mod socket;
mod stream;

pub use address::{AddressOutcome, AddressProvider, FixedAddress, GatewayAddress, Selected};
pub use challenge::{AuthChallenge, AuthScheme};
pub use client::{ClientConfig, Request, RequestAuth, Response, ServerClient};
pub use error::{Location, NetError};
pub use gateway::{
    AuthorizationUse, BasicCredentials, ExtraHeader, Gateway, GatewayCookie, GatewayCredentials,
    GatewayError,
};
pub use log::{DEFAULT_CAPACITY, LogEntry, Outcome, REDACTED, RequestLog, redact_query};
pub use reqwest::Method;
pub use reqwest::header;
pub use roaming::{
    IdentityCheck, IdentityProbe, RoamingAddresses, RoamingConfig, RoamingError, RoamingStatus,
    RuntimeHandle,
};
pub use scope::{HostScope, Transport, host_scope, transport};
pub use shape::{GatewayShape, classify};
pub use socket::{Socket, SocketClosed};
pub use stream::{
    DEFAULT_READ_TIMEOUT, STREAM_REQUEST_HEADERS, STREAM_RESPONSE_HEADERS, StreamError,
    StreamResponse,
};
pub use url::Url;
