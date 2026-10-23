//! Where a server is right now: the address seam [`ServerClient`](crate::ServerClient) asks before every request (ADR 0006, ADR 0014).

use std::future::Future;
use std::pin::Pin;

use tokio::time::Instant;
use url::Url;

/// The address a request goes to, as chosen by an [`AddressProvider`]. Its Debug output shows the position only, never the address.
#[derive(Clone, PartialEq, Eq)]
pub struct Selected {
    url: Url,
    position: Option<u8>,
    /// The race that chose it: a report about an older choice is ignored.
    pub(crate) generation: u64,
}

impl Selected {
    pub(crate) fn new(url: Url, position: Option<u8>, generation: u64) -> Self {
        Self {
            url,
            position,
            generation,
        }
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
            "Selected { position: Some(2), url: [redacted] }"
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
