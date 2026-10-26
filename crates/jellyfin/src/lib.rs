//! The Jellyfin client. Server setup turns what a user types into a base URL, probes it for a Jellyfin server, tells servers apart
//! by their id and says whether plain HTTP needs the user's confirmation (ADR 0012). Discovery finds servers on the local network
//! (ADR 0016). Sign-in covers the authorization header, password and Quick Connect sign-in, the current user and sign out
//! (ADR 0015). Behind an authentication gateway, probes classify the gateway's answer and Jellyfin's header can move to
//! `X-Emby-Authorization` (ADR 0017). Browsing lists libraries and items, the home rows, series, search and details, and sets
//! played and favourite; images are fetched at a box size through the same session (ADR 0018). Playback asks PlaybackInfo with a
//! device profile generated from the engine's capabilities, builds the stream URL, streams it for the loopback relay (ADR 0020),
//! reports playstate, and reads media segments, the session's transcoding information and the user's playback preferences (ADR 0019).
//!
//! Built on `current-net`'s [`ServerClient`](current_net::ServerClient). Wire types stay private; the public API speaks
//! `current-model`.

mod address;
mod auth;
mod browse;
mod browse_wire;
mod client;
mod discovery;
mod gateway;
mod image;
mod playback;
mod playback_wire;
mod player;
mod probe;
mod profile;
mod wire;

pub use address::{AddressError, DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT, parse_address};
pub use auth::{
    AccessToken, CLIENT_NAME, ClientIdentity, HeaderPlacement, QuickConnectSecret,
    authorization_header, device_name, platform_name,
};
pub use browse::{NEXT_UP_CUTOFF, Page, SEARCH_GROUPS, SEARCH_LIMIT, SearchGroup};
pub use client::{
    AuthError, JellyfinClient, QUICK_CONNECT_INTERVAL, QUICK_CONNECT_LIFETIME, QuickConnectCode,
    RuntimeClock, STREAM_REFUSAL_WAIT, SessionEvent, SessionHook, SignedIn, TOKEN_CHECK_TIMEOUT,
    TokenCheck, WaitClock,
};
pub use discovery::{
    Candidate, DISCOVERY_PORT, DiscoveryConfig, DiscoveryError, MAX_ADDRESSES_PER_SERVER,
    MAX_INTERFACES, MAX_REPLY_BYTES, MAX_SERVERS, Source, discover,
};
pub use gateway::{GatewayAnswer, Realm};
pub use image::{ImageError, ImageRequest};
pub use playback::{
    MAXIMUM_STREAMING_BITRATE, PlayableStream, PlaybackAnswer, PlaybackError, PlaybackRequest,
    ProgressReport, STOP_REPORT_TIMEOUT, SourceAnswer, StopReport, StreamUrl,
};
pub use player::{StreamTarget, TranscodePolicy};
pub use probe::{FoundServer, JellyfinIdentity, MINIMUM_VERSION, ProbeError, Prober, same_server};
pub use profile::{TranscodeTargets, transcode_targets};
