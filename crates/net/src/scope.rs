//! Where an address points, and whether plain HTTP to it needs the user's confirmation (ADR 0012).
//!
//! Plain HTTP is allowed silently to the device itself and to the local network, where getting a trusted certificate is hard and an
//! attacker must be on the same network; anywhere else the user confirms it once per address. The address is classified as written,
//! never by what a name resolves to.

use std::net::{Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

/// How far away an address's host is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostScope {
    /// This device: `127.0.0.0/8`, `::1`, `localhost` and `*.localhost` (RFC 6761).
    Loopback,
    /// The local network: private IPv4 (RFC 1918), link-local (`169.254.0.0/16`, `fe80::/10`), IPv6 unique local (`fc00::/7`),
    /// single-label names, and names under `.local` (RFC 6762), `.home.arpa` (RFC 8375) or `.internal` (reserved by ICANN for private
    /// use).
    Local,
    /// Anything else, including shared address space (`100.64.0.0/10`, RFC 6598): an ISP's carrier-grade NAT shares it with other
    /// subscribers, so it is not trusted as local even though mesh VPNs also use it.
    Public,
}

/// How a connection to an address is protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    /// https.
    Secure,
    /// Plain http to this device or the local network: allowed without asking.
    PlainLocal,
    /// Plain http anywhere else: allowed once the user confirms it for this address.
    PlainPublic,
}

impl Transport {
    /// Whether the user must confirm the address before it is used.
    pub fn needs_confirmation(self) -> bool {
        self == Self::PlainPublic
    }
}

/// The scope of `url`'s host. A URL without a host is [`HostScope::Public`].
pub fn host_scope(url: &Url) -> HostScope {
    match url.host() {
        Some(Host::Ipv4(address)) => ipv4_scope(address),
        Some(Host::Ipv6(address)) => ipv6_scope(address),
        Some(Host::Domain(name)) => domain_scope(name),
        None => HostScope::Public,
    }
}

/// How a connection to `url` is protected. Any scheme other than https counts as plain.
pub fn transport(url: &Url) -> Transport {
    if url.scheme() == "https" {
        Transport::Secure
    } else if host_scope(url) == HostScope::Public {
        Transport::PlainPublic
    } else {
        Transport::PlainLocal
    }
}

fn ipv4_scope(address: Ipv4Addr) -> HostScope {
    if address.is_loopback() {
        HostScope::Loopback
    } else if address.is_private() || address.is_link_local() {
        HostScope::Local
    } else {
        HostScope::Public
    }
}

fn ipv6_scope(address: Ipv6Addr) -> HostScope {
    if address.is_loopback() {
        HostScope::Loopback
    } else if let Some(v4) = address.to_ipv4_mapped() {
        ipv4_scope(v4)
    } else if address.is_unique_local() || address.is_unicast_link_local() {
        HostScope::Local
    } else {
        HostScope::Public
    }
}

fn domain_scope(name: &str) -> HostScope {
    // `url` lowercases domains; a trailing dot names the same host.
    let name = name.strip_suffix('.').unwrap_or(name);
    if name == "localhost" || name.ends_with(".localhost") {
        HostScope::Loopback
    } else if !name.contains('.')
        || [".local", ".home.arpa", ".internal"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
    {
        HostScope::Local
    } else {
        HostScope::Public
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(text: &str) -> HostScope {
        match Url::parse(text) {
            Ok(url) => host_scope(&url),
            Err(error) => panic!("{text}: {error}"),
        }
    }

    fn transport_of(text: &str) -> Transport {
        match Url::parse(text) {
            Ok(url) => transport(&url),
            Err(error) => panic!("{text}: {error}"),
        }
    }

    #[test]
    fn loopback_addresses() {
        for address in [
            "http://127.0.0.1:8096",
            "http://127.1.2.3",
            "http://[::1]:8096",
            "http://[::ffff:127.0.0.1]",
            "http://localhost:8096",
            "http://LOCALHOST.",
            "http://media.localhost",
        ] {
            assert_eq!(scope(address), HostScope::Loopback, "{address}");
        }
    }

    #[test]
    fn local_network_addresses() {
        for address in [
            "http://192.168.1.10:8096",
            "http://10.0.0.5",
            "http://172.16.0.1",
            "http://172.31.255.254",
            "http://169.254.10.20",
            "http://[fe80::1]:8096",
            "http://[fd00::10]",
            "http://[fc00::1]",
            "http://[::ffff:192.168.1.10]",
            "http://nas:8096",
            "http://nas.:8096",
            "http://media.local",
            "http://media.home.arpa",
            "http://media.internal",
        ] {
            assert_eq!(scope(address), HostScope::Local, "{address}");
        }
    }

    #[test]
    fn public_addresses() {
        for address in [
            "http://media.example.com",
            "http://8.8.8.8",
            "http://172.32.0.1",
            "http://100.64.0.1",
            "http://100.127.255.254",
            "http://[2001:db8::1]",
            "http://[::ffff:8.8.8.8]",
            "http://local.example.com",
            "http://example.localdomain",
        ] {
            assert_eq!(scope(address), HostScope::Public, "{address}");
        }
    }

    #[test]
    fn only_plain_http_to_public_addresses_needs_confirmation() {
        assert_eq!(transport_of("https://media.example.com"), Transport::Secure);
        assert_eq!(transport_of("https://192.168.1.10"), Transport::Secure);
        assert_eq!(
            transport_of("http://192.168.1.10:8096"),
            Transport::PlainLocal
        );
        assert_eq!(transport_of("http://127.0.0.1"), Transport::PlainLocal);
        assert_eq!(
            transport_of("http://media.example.com"),
            Transport::PlainPublic
        );
        assert!(transport_of("http://100.64.0.1").needs_confirmation());
        assert!(!transport_of("http://192.168.1.10").needs_confirmation());
        assert!(!transport_of("https://media.example.com").needs_confirmation());
    }
}
