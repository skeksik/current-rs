//! The loopback relay in the app (ADR 0020): one per app, made on first use; a route per playback on an address with gateway
//! credentials, forwarding through the session's own client, and the route's events back to the player's driver.
//!
//! [`SessionUpstream`] is the adapter between the relay and `current-net`: it builds the request and passes the answer on, with no
//! logic of its own. The address, its gateway credentials, the session token, roaming and `Set-Cookie` are `send_stream`'s.

use std::sync::{Arc, OnceLock};

use current_jellyfin::JellyfinClient;
use current_net::{Method, Request, RuntimeHandle, StreamResponse};
use current_relay::{
    Bytes, Relay, RelayEvent, RelayEvents, StreamMethod, Upstream, UpstreamBody, UpstreamFuture,
    UpstreamRequest, UpstreamResponse,
};
use tokio::sync::mpsc;

/// The app's one relay (ADR 0020 choice 7), made on the runtime that first needs it. It listens only while a route exists.
#[derive(Default)]
pub(crate) struct RelaySlot(OnceLock<Relay>);

impl RelaySlot {
    /// The relay; `None` outside a tokio runtime, where nothing could serve it.
    pub(crate) fn relay(&self) -> Option<Relay> {
        if let Some(relay) = self.0.get() {
            return Some(relay.clone());
        }
        let runtime = RuntimeHandle::try_current().ok()?;
        Some(self.0.get_or_init(|| Relay::new(&runtime)).clone())
    }
}

/// A route's requests, sent through the session of the item playing.
pub(crate) struct SessionUpstream(pub(crate) Arc<JellyfinClient>);

impl Upstream for SessionUpstream {
    fn send(&self, request: UpstreamRequest) -> UpstreamFuture<'_, UpstreamResponse> {
        let method = match request.method {
            StreamMethod::Get => Method::GET,
            StreamMethod::Head => Method::HEAD,
        };
        let mut outgoing = Request::new(method, request.path);
        for (name, value) in request.query {
            outgoing = outgoing.query(name, value);
        }
        for (name, value) in &request.headers {
            outgoing = outgoing.header(name.clone(), value.clone());
        }
        Box::pin(async move {
            let response = self.0.send_stream(outgoing).await?;
            Ok(UpstreamResponse {
                status: response.status(),
                headers: response.headers().clone(),
                base: response.base().clone(),
                body: Box::new(SessionBody(response)),
            })
        })
    }
}

struct SessionBody(StreamResponse);

impl UpstreamBody for SessionBody {
    fn chunk(&mut self) -> UpstreamFuture<'_, Option<Bytes>> {
        Box::pin(self.0.next_chunk())
    }
}

/// A route's events, tagged with the route's number so the driver can drop those of a route already replaced.
pub(crate) struct RouteEvents {
    pub(crate) route: u64,
    pub(crate) sender: mpsc::UnboundedSender<(u64, RelayEvent)>,
}

impl RelayEvents for RouteEvents {
    fn report(&self, event: RelayEvent) {
        // A closed receiver means the player is gone: nobody to tell.
        let _ = self.sender.send((self.route, event));
    }
}
