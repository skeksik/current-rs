//! What the relay tells the controller about a route's requests (ADR 0020, "Lifecycle"). Events carry a shape or a status, never a
//! URL, a path or a header.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use current_net::GatewayShape;

/// Something the player cannot see that the controller must act on. Each kind is reported at most once per route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayEvent {
    /// The server or a gateway refused a stream request: a 401, a 403, a redirect, or an HTML page where media was expected. The
    /// player got a 502. `status` is the answer's (200 for a page with a success status) and `shape` is `current-net`'s
    /// classification, for ADR 0017's 401 rule.
    Refused { status: u16, shape: GatewayShape },
    /// The server answered 404.
    NotFound,
    /// The server answered with another error status.
    Server { status: u16 },
    /// The server could not be reached, did not answer in time, the secure connection failed, or a body broke off.
    Network,
    /// A playlist named a URI off the server's address or outside the route's prefixes (ADR 0020 choice 5). The player got a 502.
    PlaylistElsewhere,
    /// A playlist was not an HLS playlist, held a URI the relay cannot judge, was larger than [`crate::MAX_PLAYLIST_BYTES`] (or
    /// [`crate::MAX_REWRITTEN_BYTES`] once rewritten), or came as part of itself (a 206 for a range). The player got a 502.
    PlaylistUnreadable,
    /// The upstream could not build the request for the address in use: a bug, not a server problem. The player got a 502.
    InvalidRequest,
    /// The server answered 206 with another part than the one asked for: a `Content-Range` that does not start where the request's
    /// `Range` does, none at all, or a 206 to a request without `Range`. A player would take such a body for what it asked for (a
    /// playlist's first bytes, say) that the relay judged as something else. The player got a 502.
    BadRange,
}

impl RelayEvent {
    fn bit(&self) -> u8 {
        match self {
            Self::Refused { .. } => 1,
            Self::NotFound => 1 << 1,
            Self::Server { .. } => 1 << 2,
            Self::Network => 1 << 3,
            Self::PlaylistElsewhere => 1 << 4,
            Self::PlaylistUnreadable => 1 << 5,
            Self::InvalidRequest => 1 << 6,
            Self::BadRange => 1 << 7,
        }
    }
}

/// Where a route's events go: the controller, through the app. Called on the relay's tasks; it must not block.
pub trait RelayEvents: Send + Sync {
    fn report(&self, event: RelayEvent);
}

impl RelayEvents for tokio::sync::mpsc::UnboundedSender<RelayEvent> {
    fn report(&self, event: RelayEvent) {
        // A closed receiver means nobody plays the route any more: nothing to tell.
        let _ = self.send(event);
    }
}

/// A route's sink and the kinds it has already been told.
pub(crate) struct RouteEvents {
    sink: Arc<dyn RelayEvents>,
    reported: AtomicU8,
}

impl RouteEvents {
    pub(crate) fn new(sink: Arc<dyn RelayEvents>) -> Self {
        Self {
            sink,
            reported: AtomicU8::new(0),
        }
    }

    pub(crate) fn report(&self, event: RelayEvent) {
        let bit = event.bit();
        if self.reported.fetch_or(bit, Ordering::AcqRel) & bit == 0 {
            self.sink.report(event);
        }
    }
}
