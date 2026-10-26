//! Where a server is right now: the address seam [`ServerClient`](crate::ServerClient) asks before every request (ADR 0006, ADR 0014).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::time::Instant;
use url::Url;

use crate::gateway::Gateway;

/// The address a request goes to, as chosen by an [`AddressProvider`]. Its Debug output shows the position only, never the address.
#[derive(Clone)]
pub struct Selected {
    url: Url,
    position: Option<u8>,
    /// The race that chose it: a report about an older choice is ignored.
    pub(crate) generation: u64,
    /// This address's own gateway credentials (ADR 0017), never another address's.
    gateway: Option<Arc<Gateway>>,
    /// The user accepted plain HTTP to this address (ADR 0012), so gateway credentials may go over it.
    plain_http_confirmed: bool,
}

impl PartialEq for Selected {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url
            && self.position == other.position
            && self.generation == other.generation
            && self.plain_http_confirmed == other.plain_http_confirmed
            && match (&self.gateway, &other.gateway) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for Selected {}

impl Selected {
    pub(crate) fn new(url: Url, position: Option<u8>, generation: u64) -> Self {
        Self {
            url,
            position,
            generation,
            gateway: None,
            plain_http_confirmed: false,
        }
    }

    /// The same address with its gateway credentials.
    pub(crate) fn with_gateway(
        mut self,
        gateway: Option<Arc<Gateway>>,
        plain_http_confirmed: bool,
    ) -> Self {
        self.gateway = gateway;
        self.plain_http_confirmed = plain_http_confirmed;
        self
    }

    pub(crate) fn gateway(&self) -> Option<&Arc<Gateway>> {
        self.gateway.as_ref()
    }

    pub(crate) fn plain_http_confirmed(&self) -> bool {
        self.plain_http_confirmed
    }

    /// Whether this address has gateway credentials: a gateway that holds none (the app gives every saved address one, ADR 0017)
    /// does not count.
    pub fn has_gateway(&self) -> bool {
        self.gateway
            .as_ref()
            .is_some_and(|gateway| !gateway.snapshot().0.is_empty())
    }

    /// The server's base address, including any path prefix it is served under.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The 1-based place of the address in the user's list; `None` for a server with a single [`FixedAddress`]. Logged and shown
    /// instead of the address.
    pub fn position(&self) -> Option<u8> {
        self.position
    }
}

impl std::fmt::Debug for Selected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Selected")
            .field("position", &self.position)
            .field("url", &format_args!("[redacted]"))
            .field("gateway", &self.gateway.is_some())
            .finish()
    }
}

/// What happened to a request on a [`Selected`] address. Only failures that say the address itself is unusable: a status or a decode
/// error is the server answering, and counts as [`AddressOutcome::Ok`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressOutcome {
    Ok,
    Unreachable,
    Timeout,
    Tls,
}

/// Where a server is right now (ADR 0014). The client asks before every request, so a provider can switch addresses between
/// requests.
pub trait AddressProvider: Send + Sync {
    /// The address to use now; `None` while none is known. Called before every request: cheap, at most a short lock and a clone.
    fn current(&self) -> Option<Selected>;

    /// Waits until an address is selected or choosing one failed, up to `deadline`. `None` when none was found in time.
    fn wait(
        &self,
        deadline: Instant,
    ) -> Pin<Box<dyn Future<Output = Option<Selected>> + Send + '_>>;

    /// What happened to a request on `selected`. Never blocks; may start looking for another address. Reports about an earlier choice
    /// are ignored.
    fn report(&self, selected: &Selected, outcome: AddressOutcome);
}

/// A server with one address: always selected, and reports change nothing.
#[derive(Clone, Debug)]
pub struct FixedAddress(pub Url);

impl AddressProvider for FixedAddress {
    fn current(&self) -> Option<Selected> {
        Some(Selected::new(self.0.clone(), None, 0))
    }

    fn wait(
        &self,
        _deadline: Instant,
    ) -> Pin<Box<dyn Future<Output = Option<Selected>> + Send + '_>> {
        Box::pin(std::future::ready(self.current()))
    }

    fn report(&self, _selected: &Selected, _outcome: AddressOutcome) {}
}

/// A server with one address behind an authentication gateway (ADR 0017): always selected, its requests carry `gateway`'s
/// credentials. Like the Jellyfin token, they go over plain HTTP only to the local network, or to another address once the user
/// confirmed it (`plain_http_confirmed`, ADR 0012).
pub struct GatewayAddress {
    selected: Selected,
}

impl GatewayAddress {
    pub fn new(url: Url, gateway: Arc<Gateway>, plain_http_confirmed: bool) -> Self {
        Self {
            selected: Selected::new(url, None, 0).with_gateway(Some(gateway), plain_http_confirmed),
        }
    }
}

impl std::fmt::Debug for GatewayAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("GatewayAddress")
            .field(&self.selected)
            .finish()
    }
}

impl AddressProvider for GatewayAddress {
    fn current(&self) -> Option<Selected> {
        Some(self.selected.clone())
    }

    fn wait(
        &self,
        _deadline: Instant,
    ) -> Pin<Box<dyn Future<Output = Option<Selected>> + Send + '_>> {
        Box::pin(std::future::ready(self.current()))
    }

    fn report(&self, _selected: &Selected, _outcome: AddressOutcome) {}
}

/// One address of a server with several, for the probe client of that address: always selected, with its position for the log.
#[derive(Debug)]
pub(crate) struct ProbeAddress(pub(crate) Selected);

impl AddressProvider for ProbeAddress {
    fn current(&self) -> Option<Selected> {
        Some(self.0.clone())
    }

    fn wait(
        &self,
        _deadline: Instant,
    ) -> Pin<Box<dyn Future<Output = Option<Selected>> + Send + '_>> {
        Box::pin(std::future::ready(self.current()))
    }

    fn report(&self, _selected: &Selected, _outcome: AddressOutcome) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_shows_the_position_only() {
        let selected = Selected::new(Url::parse("http://192.168.1.10:8096").unwrap(), Some(2), 7);
        assert_eq!(
            format!("{selected:?}"),
            "Selected { position: Some(2), url: [redacted], gateway: false }"
        );
    }

    #[test]
    fn a_fixed_address_is_always_selected() {
        let url = Url::parse("https://media.example.com").unwrap();
        let fixed = FixedAddress(url.clone());
        let selected = fixed.current().unwrap();
        assert_eq!(selected.url(), &url);
        assert_eq!(selected.position(), None);
        fixed.report(&selected, AddressOutcome::Unreachable);
        assert_eq!(fixed.current(), Some(selected));
    }
}
