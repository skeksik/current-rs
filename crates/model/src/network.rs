//! What the platform says about the network the device is on (ADR 0014). The UI reports these facts; the core decides what to do.
//!
//! Only facts every platform gives without a permission prompt: no Wi-Fi name. The values are the platforms', not a server's, so the
//! enums are closed.

/// Whether the network reaches anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Connectivity {
    /// No network at all.
    None,
    /// A network without internet access: the local network may still be reachable.
    LocalOnly,
    /// A captive portal wants the user to sign in to the network first.
    CaptivePortal,
    /// Internet access.
    Full,
    /// The platform does not say.
    #[default]
    Unknown,
}

/// The kind of interface the device's traffic leaves on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum InterfaceKind {
    WiFi,
    Ethernet,
    Cellular,
    Other,
    #[default]
    Unknown,
}

/// An opaque name for the network the device is on, built by the UI from what it has (a network GUID, the interface and gateways).
/// It only has to stay the same on one network and differ between networks. It can hold addresses, so it is never shown or logged:
/// it has no Display, and its Debug output is redacted.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NetworkKey(String);

impl NetworkKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

impl std::fmt::Debug for NetworkKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NetworkKey([redacted])")
    }
}

/// The network as the platform reports it, on start and on every change. `None` in an optional field means the platform does not
/// say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NetworkFacts {
    pub connectivity: Connectivity,
    /// A metered or expensive network (cellular, a hotspot, a capped plan).
    pub metered: Option<bool>,
    /// A low-data mode the user turned on.
    pub constrained: Option<bool>,
    pub interface: InterfaceKind,
    /// Whether traffic goes through a VPN.
    pub vpn: Option<bool>,
    pub network_key: Option<NetworkKey>,
}

/// An IPv4 address of one of the device's up network interfaces, with its prefix length (24 for a netmask of 255.255.255.0), as
/// the platform lists it before a local search (ADR 0016). It is the device's own address, so it is never shown or logged: its
/// Debug output is redacted.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalInterface {
    pub address: std::net::Ipv4Addr,
    /// 0 to 32; anything larger is not a valid IPv4 prefix and is ignored by its users.
    pub prefix_len: u8,
}

impl std::fmt::Debug for LocalInterface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalInterface([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_default_to_unknown() {
        let facts = NetworkFacts::default();
        assert_eq!(facts.connectivity, Connectivity::Unknown);
        assert_eq!(facts.interface, InterfaceKind::Unknown);
        assert_eq!(
            (facts.metered, facts.constrained, facts.vpn),
            (None, None, None)
        );
        assert!(facts.network_key.is_none());
    }

    #[test]
    fn debug_hides_the_network_key() {
        let facts = NetworkFacts {
            network_key: Some(NetworkKey::new("wlan0 192.168.1.10")),
            ..NetworkFacts::default()
        };
        let debug = format!("{facts:?}");
        assert!(debug.contains("NetworkKey([redacted])"), "{debug}");
        assert!(!debug.contains("192.168"), "{debug}");
    }

    #[test]
    fn debug_hides_a_local_interface() {
        let interface = LocalInterface {
            address: std::net::Ipv4Addr::new(192, 168, 1, 10),
            prefix_len: 24,
        };
        let debug = format!("{interface:?}");
        assert_eq!(debug, "LocalInterface([redacted])");
    }
}
