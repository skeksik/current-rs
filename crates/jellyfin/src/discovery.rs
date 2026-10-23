//! Finding Jellyfin servers on the local network (ADR 0016).
//!
//! Jellyfin answers a UDP datagram containing `who is JellyfinServer?` on port 7359 with a small JSON reply sent back to the
//! sender (`AutoDiscoveryHost`, Jellyfin 12.2). Any host on the network can answer, so replies are untrusted: they are bounded and
//! parsed strictly here, and what they yield are only [`Candidate`]s, which the caller probes ([`Prober`](crate::Prober)) and
//! checks against the reply's id before showing them.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use current_model::ServerId;
use current_net::Url;
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

/// Where and how long to search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryConfig {
    /// Where the request goes. By default only the limited broadcast address, `255.255.255.255:7359`: the standard library cannot
    /// list interfaces, so per-interface broadcast addresses would come from the platform (ADR 0016).
    pub targets: Vec<SocketAddr>,
    /// How long replies are collected after the first send.
    pub window: Duration,
    /// When to send the request again, if inside the window.
    pub resend_after: Option<Duration>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            targets: vec![SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT))],
            window: DEFAULT_WINDOW,
            resend_after: Some(DEFAULT_RESEND_AFTER),
        }
    }
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

/// Sends the discovery request to every target and collects the replies until the window ends, [`MAX_SERVERS`] servers answered
/// or too many datagrams came. Replies claiming the same server id are merged: the first one's name is kept and every reply's
/// addresses are added in arrival order (deduplicated, at most [`MAX_ADDRESSES_PER_SERVER`]), so a spoofed reply that arrives first
/// cannot hide the real server's. Malformed, oversized and invalid replies are skipped.
///
/// # Errors
/// [`DiscoveryError`] when the socket cannot be opened or the request could be sent to no target.
pub async fn discover(config: &DiscoveryConfig) -> Result<Vec<Candidate>, DiscoveryError> {
    let start = Instant::now();
    let deadline = start + config.window;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.set_broadcast(true)?;
    send_all(&socket, &config.targets).await?;

    let mut resend_at = config
        .resend_after
        .map(|after| start + after)
        .filter(|at| *at < deadline);
    let mut buffer = vec![0u8; MAX_REPLY_BYTES + 1];
    let mut found: Vec<Candidate> = Vec::new();
    let mut datagrams = 0;
    while datagrams < MAX_DATAGRAMS && found.len() < MAX_SERVERS {
        let wake = resend_at.unwrap_or(deadline);
        match tokio::time::timeout_at(wake, socket.recv_from(&mut buffer)).await {
            Err(_) => match resend_at.take() {
                Some(_) => {
                    // The first send worked, so a failure now is passing (the network changed): the replies to the first count.
                    let _ = send_all(&socket, &config.targets).await;
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

/// Sends the request to every target. Succeeds if one send did; otherwise fails with the first error.
async fn send_all(socket: &UdpSocket, targets: &[SocketAddr]) -> Result<(), DiscoveryError> {
    let mut first_error = None;
    let mut sent = false;
    for target in targets {
        match socket.send_to(REQUEST, target).await {
            Ok(_) => sent = true,
            Err(error) => {
                first_error.get_or_insert(DiscoveryError::from(error));
            }
        }
    }
    match (sent, first_error) {
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
        DiscoveryConfig {
            targets,
            window: Duration::from_millis(300),
            resend_after: None,
        }
    }

    fn addresses(candidate: &Candidate) -> Vec<String> {
        candidate.addresses().iter().map(Url::to_string).collect()
    }

    #[test]
    fn the_default_is_the_limited_broadcast_inside_windows_window() {
        let config = DiscoveryConfig::default();
        assert_eq!(config.targets, ["255.255.255.255:7359".parse().unwrap()]);
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
