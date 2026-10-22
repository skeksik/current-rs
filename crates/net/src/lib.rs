//! The HTTP client for one server: its address comes from an [`AddressProvider`], every request has a timeout, errors are typed by
//! what the UI does about them, and every request goes into a redacted, bounded [`RequestLog`] (ADR 0006).
//!
//! Service crates (Jellyfin, Seerr, Stash) build on [`ServerClient`] and add their credentials through [`RequestAuth`].

mod client;
mod error;
mod log;

pub use client::{
    AddressProvider, ClientConfig, FixedAddress, Request, RequestAuth, Response, ServerClient,
};
pub use error::NetError;
pub use log::{DEFAULT_CAPACITY, LogEntry, Outcome, REDACTED, RequestLog, redact_query};
pub use reqwest::Method;
pub use reqwest::header;
pub use url::Url;
