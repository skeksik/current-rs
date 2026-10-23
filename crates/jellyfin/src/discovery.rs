//! Finding Jellyfin servers on the local network (ADR 0016).
//!
//! Jellyfin answers a UDP datagram containing `who is JellyfinServer?` on port 7359 with a small JSON reply sent back to the
//! sender (`AutoDiscoveryHost`, Jellyfin 12.2). Any host on the network can answer, so replies are untrusted: they are bounded and
//! parsed strictly here, and what they yield are only [`Candidate`]s, which the caller probes ([`Prober`](crate::Prober)) and
//! checks against the reply's id before showing them.

use std::fmt;
use std::future::poll_fn;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::task::Poll;
use std::time::Duration;

use current_model::{LocalInterface, ServerId};
use current_net::Url;
use tokio::io::ReadBuf;
use tokio::net::UdpSocket;
use tokio::time::Instant;
use url::Host;

use crate::address::parse_address;
use crate::wire;

/// The UDP port Jellyfin listens on for discovery (`AutoDiscoveryHost.PortNumber`).
pub const DISCOVERY_PORT: u16 = 7359;

/// What the server looks for in a datagram (`AutoDiscoveryHost`, compared without case).
const REQUEST: &[u8] = b"who is JellyfinServer?";

/// How long replies are collected. Windows Defender Firewall accepts unicast replies to a broadcast for "as long as three seconds"
/// after it is sent and blocks later ones (ADR 0016), so the window stays well under that.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(2);

/// When the request is sent a second time, against a lost datagram. Still inside Windows' three seconds after the first send.
pub const DEFAULT_RESEND_AFTER: Duration = Duration::from_millis(500);

/// The largest reply read. A real one is about 200 bytes; a larger datagram is dropped whole.
pub const MAX_REPLY_BYTES: usize = 2048;

/// The most servers one search returns: replies after that are not read.
pub const MAX_SERVERS: usize = 32;

/// The most datagrams one search reads, valid or not, so a flood of junk cannot keep it busy.
const MAX_DATAGRAMS: usize = 256;

/// The longest server id accepted. Jellyfin's is 32 hex digits.
const MAX_ID_CHARS: usize = 64;

/// The longest server name kept, in characters; longer names are cut.
const MAX_NAME_CHARS: usize = 256;

/// The most addresses kept per server id, from all replies claiming it.
pub const MAX_ADDRESSES_PER_SERVER: usize = 4;

/// The most local interfaces a search sends from, after dropping unusable and repeated ones; later ones are ignored.
pub const MAX_INTERFACES: usize = 16;

/// Where and how long to search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryConfig {
    /// The sockets the request is sent from, each to its own targets. By default one unbound socket to the limited broadcast
    /// address, `255.255.255.255:7359`; [`with_interfaces`](Self::with_interfaces) adds one per local interface (ADR 0016).
    pub sources: Vec<Source>,
    /// How long replies are collected after the first send.
    pub window: Duration,
    /// When to send the request again, if inside the window.
    pub resend_after: Option<Duration>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            sources: vec![Source {
                bind: Ipv4Addr::UNSPECIFIED,
                targets: vec![SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT))],
            }],
            window: DEFAULT_WINDOW,
            resend_after: Some(DEFAULT_RESEND_AFTER),
        }
    }
}

impl DiscoveryConfig {
    /// This configuration plus one source per usable interface in `interfaces`, bound to its address and sending to its subnet's
    /// broadcast address and to `255.255.255.255`, on port 7359 (ADR 0016). The existing sources stay, so the unbound socket
    /// still covers an interface the platform left out.
    ///
    /// Loopback, unspecified, multicast and broadcast addresses and prefixes over 32 are dropped, then repeated addresses, then
    /// everything after the first [`MAX_INTERFACES`]. Link-local addresses (169.254/16) are kept: Jellyfin answers on every
    /// address, and on a link without DHCP they are the only way to reach it.
    #[must_use]
    pub fn with_interfaces(mut self, interfaces: &[LocalInterface]) -> Self {
        let mut addresses: Vec<Ipv4Addr> = Vec::new();
        for interface in interfaces.iter().filter(|interface| usable(interface)) {
            if addresses.len() >= MAX_INTERFACES {
                break;
            }
            if addresses.contains(&interface.address) {
                continue;
            }
            addresses.push(interface.address);
            let mut targets = Vec::with_capacity(2);
            if let Some(broadcast) = subnet_broadcast(interface) {
                targets.push(SocketAddr::from((broadcast, DISCOVERY_PORT)));
            }
            let limited = SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT));
            if !targets.contains(&limited) {
                targets.push(limited);
            }
            self.sources.push(Source {
                bind: interface.address,
                targets,
            });
        }
        self
    }
}

/// One socket of a search: the local address it is bound to and where it sends the request. Debug leaves out the addresses.
#[derive(Clone, PartialEq, Eq)]
pub struct Source {
    /// The local address to bind to, on any port; `0.0.0.0` lets the system choose.
    pub bind: Ipv4Addr,
    pub targets: Vec<SocketAddr>,
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Source")
            .field("unbound", &self.bind.is_unspecified())
            .field(
                "targets",
                &format_args!("[{} redacted]", self.targets.len()),
            )
            .finish()
    }
}

/// Whether a search may send from `interface`.
fn usable(interface: &LocalInterface) -> bool {
    let address = interface.address;
    interface.prefix_len <= 32
        && !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && !address.is_broadcast()
}

/// The broadcast address of `interface`'s subnet, or `None` when the prefix leaves it none: /31 and /32 links have no broadcast
/// address, and /0 would be the limited broadcast itself.
fn subnet_broadcast(interface: &LocalInterface) -> Option<Ipv4Addr> {
    if !(1..=30).contains(&interface.prefix_len) {
        return None;
    }
    let host_bits = u32::MAX >> interface.prefix_len;
    Some(Ipv4Addr::from(u32::from(interface.address) | host_bits))
}

/// A server that answered the search. Untrusted: probe one of its addresses and check the answer has the same id before showing
/// it. Debug leaves out the addresses.
#[derive(Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The id the reply claims.
    pub id: ServerId,
    /// The name the reply claims, without control characters and cut to 256 characters. A hint only: show the probed name.
    pub name: String,
    addresses: Vec<Url>,
}

impl Candidate {
    /// The base URLs to probe, in order, from every reply claiming this id (at most [`MAX_ADDRESSES_PER_SERVER`]). For each reply:
    /// when its address is an IP other than the one the reply came from, an address at the reply's source comes first (a server in
    /// a container may name an address this device cannot reach), then the reply's own. Never empty.
    pub fn addresses(&self) -> &[Url] {
        &self.addresses
    }
}

impl fmt::Debug for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Candidate")
            .field("id", &self.id)
            .field("name", &self.name)
            .field(
                "addresses",
                &format_args!("[{} redacted]", self.addresses.len()),
            )
            .finish()
    }
}

/// Why a search could not run. No variant holds an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    /// The system refused: on Apple platforms, typically the Local Network privilege not (yet) granted.
    #[error("this device does not allow Current to search the local network")]
    NotPermitted,
    /// No network to search: not connected, or no route for the broadcast.
    #[error("this device is not connected to a local network")]
    NoNetwork,
    /// Any other failure of the socket.
    #[error("searching the local network failed")]
    Failed,
}

impl From<io::Error> for DiscoveryError {
    /// Sorts by the error's kind only, never its text.
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::PermissionDenied => Self::NotPermitted,
            // A denied Local Network privilege on macOS may also surface as "no route to host" (not verified).
            io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkDown
            | io::ErrorKind::AddrNotAvailable => Self::NoNetwork,
            _ => Self::Failed,
        }
    }
}

/// Sends the discovery request from every source to its targets and collects the replies on all of them until the window ends,
/// [`MAX_SERVERS`] servers answered or too many datagrams came (both limits shared by all sources). Replies claiming the same
/// server id are merged: the first one's name is kept and every reply's addresses are added in arrival order (deduplicated, at
/// most [`MAX_ADDRESSES_PER_SERVER`]), so a spoofed reply that arrives first cannot hide the real server's. Malformed, oversized
/// and invalid replies are skipped. A source that cannot be bound, or whose every send fails, is left out.
///
/// # Errors
/// [`DiscoveryError`] when every source failed: [`NotPermitted`](DiscoveryError::NotPermitted) if one of them was refused that
/// way, otherwise the first failure.
pub async fn discover(config: &DiscoveryConfig) -> Result<Vec<Candidate>, DiscoveryError> {
    let start = Instant::now();
    let deadline = start + config.window;
    let mut sockets: Vec<(UdpSocket, &[SocketAddr])> = Vec::with_capacity(config.sources.len());
    let mut failure = Failure::default();
    for source in &config.sources {
        match open(source).await {
            Ok(socket) => sockets.push((socket, &source.targets)),
            Err(error) => failure.note(error),
        }
    }
    if sockets.is_empty() {
        return match failure.0 {
            Some(error) => Err(error),
            None => Ok(Vec::new()),
        };
    }

    let mut resend_at = config
        .resend_after
        .map(|after| start + after)
        .filter(|at| *at < deadline);
    let mut buffer = vec![0u8; MAX_REPLY_BYTES + 1];
    let mut found: Vec<Candidate> = Vec::new();
    let mut datagrams = 0;
    // The socket polled first next time, so that a busy socket cannot starve the others.
    let mut turn = 0;
    while datagrams < MAX_DATAGRAMS && found.len() < MAX_SERVERS {
        let wake = resend_at.unwrap_or(deadline);
        match tokio::time::timeout_at(wake, recv_any(&sockets, &mut turn, &mut buffer)).await {
            Err(_) => match resend_at.take() {
                Some(_) => {
                    for (socket, targets) in &sockets {
                        // The first send worked, so a failure now is passing (the network changed): the replies to the first count.
                        let _ = send_all(socket, targets).await;
                    }
                }
                None => break,
            },
            // For example a port-unreachable report surfacing as a reset on Windows: skip it like a bad reply.
            Ok(Err(_)) => datagrams += 1,
            Ok(Ok((len, source))) => {
                datagrams += 1;
                if len > MAX_REPLY_BYTES {
                    continue;
                }
                if let Some(candidate) = parse_reply(&buffer[..len], source.ip()) {
                    merge(&mut found, candidate);
                }
            }
        }
    }
    Ok(found)
}

/// A socket bound to `source`'s address, allowed to broadcast, that has sent the request to at least one of its targets.
async fn open(source: &Source) -> Result<UdpSocket, DiscoveryError> {
    let socket = UdpSocket::bind((source.bind, 0)).await?;
    socket.set_broadcast(true)?;
    send_all(&socket, &source.targets).await?;
    Ok(socket)
}

/// The next datagram on any of `sockets`, polling them in turn from `turn`, then moving `turn` past the one that had it.
async fn recv_any(
    sockets: &[(UdpSocket, &[SocketAddr])],
    turn: &mut usize,
    buffer: &mut [u8],
) -> io::Result<(usize, SocketAddr)> {
    poll_fn(|cx| {
        let count = sockets.len();
        for offset in 0..count {
            let index = (*turn + offset) % count;
            let mut read = ReadBuf::new(buffer);
            if let Poll::Ready(result) = sockets[index].0.poll_recv_from(cx, &mut read) {
                *turn = (index + 1) % count;
                return Poll::Ready(result.map(|source| (read.filled().len(), source)));
            }
        }
        Poll::Pending
    })
    .await
}

/// The error a search reports when nothing could be sent: [`DiscoveryError::NotPermitted`] if any failure was that, since it is
/// the one the user can act on, otherwise the first.
#[derive(Default)]
struct Failure(Option<DiscoveryError>);

impl Failure {
    fn note(&mut self, error: DiscoveryError) {
        if self.0.is_none() || error == DiscoveryError::NotPermitted {
            self.0 = Some(error);
        }
    }
}

/// Adds `candidate` to `found`, or its new addresses to the known candidate with the same id.
fn merge(found: &mut Vec<Candidate>, candidate: Candidate) {
    let Some(known) = found.iter_mut().find(|known| known.id == candidate.id) else {
        found.push(candidate);
        return;
    };
    for address in candidate.addresses {
        if known.addresses.len() >= MAX_ADDRESSES_PER_SERVER {
            break;
        }
        if !known.addresses.contains(&address) {
            known.addresses.push(address);
        }
    }
}

/// Sends the request to every target. Succeeds if one send did (or there was none to send to); otherwise fails as [`Failure`]
/// says.
async fn send_all(socket: &UdpSocket, targets: &[SocketAddr]) -> Result<(), DiscoveryError> {
    let mut failure = Failure::default();
    let mut sent = false;
    for target in targets {
        match socket.send_to(REQUEST, target).await {
            Ok(_) => sent = true,
            Err(error) => failure.note(DiscoveryError::from(error)),
        }
    }
    match (sent, failure.0) {
        (false, Some(error)) => Err(error),
        _ => Ok(()),
    }
}

/// A reply as a candidate, or `None` if it is not a valid reply: JSON (hence UTF-8) holding an object with a valid `Id` and an
/// `Address` that [`parse_address`] accepts.
fn parse_reply(bytes: &[u8], source: IpAddr) -> Option<Candidate> {
    let info: wire::ServerDiscoveryInfo = serde_json::from_slice(bytes).ok()?;
    let id = info.id.filter(|id| valid_id(id))?;
    let address = parse_address(&info.address?).ok()?.into_iter().next()?;
    let mut addresses = Vec::with_capacity(2);
    addresses.extend(at_source(&address, source));
    addresses.push(address);
    Some(Candidate {
        id: ServerId::new(id),
        name: clean_name(info.name.as_deref().unwrap_or_default()),
        addresses,
    })
}

/// 1 to 64 ASCII letters, digits or hyphens: a GUID in any of its usual forms.
fn valid_id(id: &str) -> bool {
    (1..=MAX_ID_CHARS).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_NAME_CHARS)
        .collect()
}

/// `address` with its host replaced by `source`, when the host is an IP address other than `source`.
fn at_source(address: &Url, source: IpAddr) -> Option<Url> {
    let host: IpAddr = match address.host()? {
        Host::Ipv4(ip) => ip.into(),
        Host::Ipv6(ip) => ip.into(),
        Host::Domain(_) => return None,
    };
    if host == source || source.is_unspecified() {
        return None;
    }
    let mut url = address.clone();
    url.set_ip_host(source).ok()?;
    Some(url)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn reply(address: &str, id: &str, name: &str) -> Vec<u8> {
        format!(r#"{{"Address":"{address}","Id":"{id}","Name":"{name}","EndpointAddress":null}}"#)
            .into_bytes()
    }

    /// A discovery stand-in on 127.0.0.1: for each request, after skipping the first `ignore`, it sends every datagram in
    /// `replies`. Returns its address and how many requests it got.
    async fn stand_in(replies: Vec<Vec<u8>>, ignore: usize) -> (SocketAddr, Arc<AtomicUsize>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 512];
            while let Ok((len, from)) = socket.recv_from(&mut buffer).await {
                assert_eq!(&buffer[..len], REQUEST);
                if counter.fetch_add(1, Ordering::SeqCst) < ignore {
                    continue;
                }
                for reply in &replies {
                    let _ = socket.send_to(reply, from).await;
                }
            }
        });
        (address, requests)
    }

    fn config(targets: Vec<SocketAddr>) -> DiscoveryConfig {
        sources(vec![source(Ipv4Addr::UNSPECIFIED, targets)])
    }

    fn sources(sources: Vec<Source>) -> DiscoveryConfig {
        DiscoveryConfig {
            sources,
            window: Duration::from_millis(300),
            resend_after: None,
        }
    }

    fn source(bind: Ipv4Addr, targets: Vec<SocketAddr>) -> Source {
        Source { bind, targets }
    }

    fn interface(address: [u8; 4], prefix_len: u8) -> LocalInterface {
        LocalInterface {
            address: Ipv4Addr::from(address),
            prefix_len,
        }
    }

    fn limited() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT))
    }

    fn port(address: [u8; 4]) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::from(address), DISCOVERY_PORT))
    }

    /// TEST-NET-1 (RFC 5737): never an address of this machine, so binding to it fails.
    const NOT_LOCAL: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

    fn addresses(candidate: &Candidate) -> Vec<String> {
        candidate.addresses().iter().map(Url::to_string).collect()
    }

    #[test]
    fn the_default_is_the_limited_broadcast_inside_windows_window() {
        let config = DiscoveryConfig::default();
        assert_eq!(
            config.sources,
            [source(
                Ipv4Addr::UNSPECIFIED,
                vec!["255.255.255.255:7359".parse().unwrap()]
            )]
        );
        assert!(config.window < Duration::from_secs(3));
        assert!(config.resend_after.unwrap() < config.window);
    }

    #[tokio::test]
    async fn a_reply_becomes_a_candidate() {
        let (target, requests) =
            stand_in(vec![reply("http://127.0.0.1:8096", ID, "Test Server")], 0).await;
        let found = discover(&config(vec![target])).await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, ServerId::new(ID));
        assert_eq!(found[0].name, "Test Server");
        assert_eq!(addresses(&found[0]), ["http://127.0.0.1:8096/"]);
    }

    #[tokio::test]
    async fn replies_from_one_server_are_merged() {
        let (one, _) = stand_in(vec![reply("http://127.0.0.1:8096", ID, "First")], 0).await;
        let (two, _) = stand_in(
            vec![
                reply("http://127.0.0.1:8096", ID, "Second"),
                reply("http://127.0.0.1:8097", "other", "Other"),
            ],
            0,
        )
        .await;
        let found = discover(&config(vec![one, two])).await.unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(
            found.iter().filter(|c| c.id == ServerId::new(ID)).count(),
            1
        );
    }

    #[tokio::test]
    async fn a_spoofed_reply_first_does_not_hide_the_real_address() {
        let replies = vec![
            // A name, so no address at the reply's source is added for it.
            reply("https://media.example.com", ID, "Spoof"),
            reply("http://127.0.0.1:8096", ID, "Real"),
            reply("http://127.0.0.1:8096", ID, "Real again"),
        ]
        .into_iter()
        .chain((0..8).map(|n| reply(&format!("http://127.0.0.1:{}", 9000 + n), ID, "More")))
        .collect();
        let (target, _) = stand_in(replies, 0).await;
        let found = discover(&config(vec![target])).await.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "Spoof");
        assert_eq!(
            addresses(&found[0]),
            [
                "https://media.example.com/",
                "http://127.0.0.1:8096/",
                "http://127.0.0.1:9000/",
                "http://127.0.0.1:9001/",
            ]
        );
    }

    #[tokio::test]
    async fn bad_replies_are_skipped() {
        // Valid but over the size limit.
        let oversized = reply("http://127.0.0.1:8096", "big", &"x".repeat(MAX_REPLY_BYTES));
        let bad = vec![
            b"not json".to_vec(),
            vec![0xff, 0xfe, b'{', b'}'],
            b"null".to_vec(),
            br#"["http://127.0.0.1:8096"]"#.to_vec(),
            br#"{"Address":"http://127.0.0.1:8096","Name":"no id"}"#.to_vec(),
            br#"{"Id":"noaddress","Name":"no address"}"#.to_vec(),
            reply("http://127.0.0.1:8096", "", "empty id"),
            reply("http://127.0.0.1:8096", "bad id!", "bad id"),
            reply("http://127.0.0.1:8096", &"a".repeat(65), "long id"),
            reply("ftp://127.0.0.1:8096", "ftp", "ftp"),
            reply("http://user:pass@127.0.0.1:8096", "creds", "credentials"),
            reply("http://", "nohost", "no host"),
            oversized,
            br#"{"Address":"http://127.0.0.1:8096","Id":"trailing","Name":"x"} junk"#.to_vec(),
            reply("http://127.0.0.1:8096", ID, "Good"),
        ];
        let (target, _) = stand_in(bad, 0).await;
        let found = discover(&config(vec![target])).await.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "Good");
    }

    #[tokio::test]
    async fn names_are_cleaned_and_cut() {
        let long = format!("  Test\\u0007 Server {}", "n".repeat(400));
        let (target, _) = stand_in(vec![reply("http://127.0.0.1:8096", ID, &long)], 0).await;
        let found = discover(&config(vec![target])).await.unwrap();
        assert!(found[0].name.starts_with("Test Server nnn"));
        assert_eq!(found[0].name.chars().count(), MAX_NAME_CHARS);
    }

    #[tokio::test]
    async fn no_more_than_the_maximum_servers() {
        let replies = (0..MAX_SERVERS + 8)
            .map(|n| reply("http://127.0.0.1:8096", &format!("id{n}"), "Server"))
            .collect();
        let (target, _) = stand_in(replies, 0).await;
        let found = discover(&config(vec![target])).await.unwrap();
        assert_eq!(found.len(), MAX_SERVERS);
    }

    #[tokio::test]
    async fn the_search_ends_with_its_window() {
        let (target, requests) = stand_in(Vec::new(), usize::MAX).await;
        let started = std::time::Instant::now();
        let found = discover(&config(vec![target])).await.unwrap();
        let took = started.elapsed();
        assert!(found.is_empty());
        assert!(took >= Duration::from_millis(300), "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_lost_request_is_sent_again() {
        let (target, requests) =
            stand_in(vec![reply("http://127.0.0.1:8096", ID, "Test Server")], 1).await;
        let config = DiscoveryConfig {
            resend_after: Some(Duration::from_millis(100)),
            ..config(vec![target])
        };
        let found = discover(&config).await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(found.len(), 1);
    }

    #[tokio::test]
    async fn the_source_comes_first_when_the_reply_names_another_ip() {
        let (target, _) = stand_in(
            vec![
                reply("http://192.168.1.10:8096/jellyfin", "a", "Other IP"),
                reply("https://media.example.com", "b", "Name"),
            ],
            0,
        )
        .await;
        let found = discover(&config(vec![target])).await.unwrap();
        let by_id = |id: &str| {
            found
                .iter()
                .find(|c| c.id == ServerId::new(id))
                .map(addresses)
                .unwrap()
        };
        assert_eq!(
            by_id("a"),
            [
                "http://127.0.0.1:8096/jellyfin",
                "http://192.168.1.10:8096/jellyfin"
            ]
        );
        assert_eq!(by_id("b"), ["https://media.example.com/"]);
    }

    #[tokio::test]
    async fn debug_output_holds_no_address() {
        let (target, _) = stand_in(vec![reply("http://192.168.1.10:8096", ID, "Test")], 0).await;
        let found = discover(&config(vec![target])).await.unwrap();
        let text = format!("{found:?}");
        assert!(!text.contains("127.0.0.1"), "{text}");
        assert!(!text.contains("192.168.1.10"), "{text}");
        assert!(!text.contains("8096"), "{text}");
        assert!(text.contains("[2 redacted]"), "{text}");
    }

    #[test]
    fn each_interface_adds_a_source_to_its_subnet_and_the_limited_broadcast() {
        let config = DiscoveryConfig::default().with_interfaces(&[
            interface([192, 168, 1, 10], 24),
            interface([198, 51, 100, 7], 22),
        ]);
        assert_eq!(
            config.sources,
            [
                DiscoveryConfig::default().sources[0].clone(),
                source(
                    Ipv4Addr::new(192, 168, 1, 10),
                    vec![port([192, 168, 1, 255]), limited()]
                ),
                source(
                    Ipv4Addr::new(198, 51, 100, 7),
                    vec![port([198, 51, 103, 255]), limited()]
                ),
            ]
        );
    }

    #[test]
    fn no_interfaces_leave_the_unbound_source_alone() {
        let config = DiscoveryConfig::default().with_interfaces(&[]);
        assert_eq!(config, DiscoveryConfig::default());
    }

    #[test]
    fn unusable_and_repeated_interfaces_are_dropped() {
        let config = DiscoveryConfig::default().with_interfaces(&[
            interface([127, 0, 0, 1], 8),
            interface([0, 0, 0, 0], 0),
            interface([224, 0, 0, 251], 24),
            interface([255, 255, 255, 255], 32),
            interface([192, 168, 1, 10], 33),
            interface([192, 168, 1, 10], 24),
            interface([192, 168, 1, 10], 16),
            interface([169, 254, 10, 20], 16),
        ]);
        let binds: Vec<Ipv4Addr> = config.sources.iter().map(|source| source.bind).collect();
        assert_eq!(
            binds,
            [
                Ipv4Addr::UNSPECIFIED,
                Ipv4Addr::new(192, 168, 1, 10),
                Ipv4Addr::new(169, 254, 10, 20)
            ]
        );
        assert_eq!(
            config.sources[1].targets,
            [port([192, 168, 1, 255]), limited()]
        );
        assert_eq!(
            config.sources[2].targets,
            [port([169, 254, 255, 255]), limited()]
        );
    }

    #[test]
    fn links_without_a_subnet_broadcast_send_to_the_limited_one_only() {
        let config = DiscoveryConfig::default().with_interfaces(&[
            interface([192, 168, 1, 10], 32),
            interface([198, 51, 100, 10], 31),
            interface([203, 0, 113, 10], 0),
            // Its subnet broadcast is the limited one, sent once.
            interface([255, 255, 255, 1], 24),
        ]);
        for source in &config.sources {
            assert_eq!(source.targets, [limited()]);
        }
        assert_eq!(config.sources.len(), 5);
    }

    #[test]
    fn interfaces_are_capped() {
        let many: Vec<LocalInterface> = (1..=40).map(|n| interface([203, 0, 113, n], 24)).collect();
        let config = DiscoveryConfig::default().with_interfaces(&many);
        assert_eq!(config.sources.len(), 1 + MAX_INTERFACES);
        assert_eq!(
            config.sources.last().unwrap().bind,
            Ipv4Addr::new(203, 0, 113, 16)
        );
    }

    #[test]
    fn config_debug_holds_no_address() {
        let config =
            DiscoveryConfig::default().with_interfaces(&[interface([192, 168, 1, 10], 24)]);
        let text = format!("{config:?}");
        assert!(!text.contains("192.168"), "{text}");
        assert!(!text.contains("255.255"), "{text}");
        assert!(!text.contains("7359"), "{text}");
        assert!(text.contains("[2 redacted]"), "{text}");
    }

    #[tokio::test]
    async fn replies_on_every_source_are_collected_and_merged() {
        // Each source reaches only its own stand-in, so every id found proves its socket was read.
        let mut stand_ins = Vec::new();
        for id in ["a", "b", "c"] {
            stand_ins.push(stand_in(vec![reply("http://127.0.0.1:8096", id, id)], 0).await);
        }
        let (again, _) = stand_in(vec![reply("http://127.0.0.1:8097", "a", "a")], 0).await;
        let mut list: Vec<Source> = stand_ins
            .iter()
            .map(|(target, _)| source(Ipv4Addr::LOCALHOST, vec![*target]))
            .collect();
        list[2].bind = Ipv4Addr::UNSPECIFIED;
        list.push(source(Ipv4Addr::LOCALHOST, vec![again]));
        let found = discover(&sources(list)).await.unwrap();
        for (_, requests) in &stand_ins {
            assert_eq!(requests.load(Ordering::SeqCst), 1);
        }
        let mut ids: Vec<&str> = found.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["a", "b", "c"], "{found:?}");
        let a = found.iter().find(|c| c.id == ServerId::new("a")).unwrap();
        assert_eq!(
            addresses(a),
            ["http://127.0.0.1:8096/", "http://127.0.0.1:8097/"]
        );
    }

    #[tokio::test]
    async fn a_source_whose_every_send_fails_does_not_fail_the_search() {
        let (target, _) =
            stand_in(vec![reply("http://127.0.0.1:8096", ID, "Test Server")], 0).await;
        // An IPv6 target from an IPv4 socket: every system rejects the send.
        let unsendable = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, DISCOVERY_PORT));
        let found = discover(&sources(vec![
            source(Ipv4Addr::LOCALHOST, vec![unsendable]),
            source(Ipv4Addr::LOCALHOST, vec![target]),
        ]))
        .await
        .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        let alone = discover(&sources(vec![source(
            Ipv4Addr::LOCALHOST,
            vec![unsendable],
        )]))
        .await;
        assert!(alone.is_err(), "{alone:?}");
    }

    #[tokio::test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "macOS configures only 127.0.0.1 on its loopback interface by default, so 127.0.0.2 cannot be bound"
    )]
    async fn each_source_sends_from_its_own_address() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = socket.local_addr().unwrap();
        let seen = tokio::spawn(async move {
            let mut buffer = [0u8; 512];
            let mut from = Vec::new();
            while from.len() < 2 {
                let (_, sender) = socket.recv_from(&mut buffer).await.unwrap();
                let reply = reply("http://127.0.0.1:8096", &format!("id{}", from.len()), "S");
                socket.send_to(&reply, sender).await.unwrap();
                from.push(sender.ip());
            }
            from
        });
        let found = discover(&sources(vec![
            source(Ipv4Addr::new(127, 0, 0, 1), vec![target]),
            source(Ipv4Addr::new(127, 0, 0, 2), vec![target]),
        ]))
        .await
        .unwrap();
        let mut from = seen.await.unwrap();
        from.sort();
        assert_eq!(
            from,
            [IpAddr::from([127, 0, 0, 1]), IpAddr::from([127, 0, 0, 2])]
        );
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[tokio::test]
    async fn a_source_that_cannot_bind_does_not_fail_the_search() {
        let (target, _) =
            stand_in(vec![reply("http://127.0.0.1:8096", ID, "Test Server")], 0).await;
        let found = discover(&sources(vec![
            source(NOT_LOCAL, vec![target]),
            source(Ipv4Addr::LOCALHOST, vec![target]),
        ]))
        .await
        .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[tokio::test]
    async fn the_search_fails_when_every_source_failed() {
        let target = SocketAddr::from(([127, 0, 0, 1], DISCOVERY_PORT));
        let error = discover(&sources(vec![
            source(NOT_LOCAL, vec![target]),
            source(NOT_LOCAL, vec![target]),
        ]))
        .await
        .unwrap_err();
        assert_eq!(error, DiscoveryError::NoNetwork);
    }

    #[test]
    fn not_permitted_wins_over_other_failures() {
        let mut failure = Failure::default();
        failure.note(DiscoveryError::NoNetwork);
        failure.note(DiscoveryError::Failed);
        assert_eq!(failure.0, Some(DiscoveryError::NoNetwork));
        failure.note(DiscoveryError::NotPermitted);
        failure.note(DiscoveryError::Failed);
        assert_eq!(failure.0, Some(DiscoveryError::NotPermitted));
    }

    #[tokio::test]
    async fn a_busy_source_does_not_starve_the_others() {
        let junk = (0..MAX_DATAGRAMS + 64).map(|_| b"junk".to_vec()).collect();
        let (noisy, _) = stand_in(junk, 0).await;
        let (quiet, _) = stand_in(vec![reply("http://127.0.0.1:8096", ID, "Test Server")], 0).await;
        let found = discover(&sources(vec![
            source(Ipv4Addr::LOCALHOST, vec![noisy]),
            source(Ipv4Addr::LOCALHOST, vec![quiet]),
        ]))
        .await
        .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn io_errors_map_by_kind() {
        let error = |kind| DiscoveryError::from(io::Error::from(kind));
        assert_eq!(
            error(io::ErrorKind::PermissionDenied),
            DiscoveryError::NotPermitted
        );
        assert_eq!(
            error(io::ErrorKind::NetworkUnreachable),
            DiscoveryError::NoNetwork
        );
        assert_eq!(
            error(io::ErrorKind::AddrNotAvailable),
            DiscoveryError::NoNetwork
        );
        assert_eq!(error(io::ErrorKind::Other), DiscoveryError::Failed);
    }
}
