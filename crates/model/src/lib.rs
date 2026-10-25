//! Domain models shared by every Current client. Pure Rust: no UI, no I/O.
//!
//! The types are plain records and enums with owned fields (no generics or lifetimes in their public shape) so that they can cross the
//! UniFFI bridge later (ADR 0002, ADR 0005).

mod appearance;
mod artwork;
mod capabilities;
mod engine;
mod identity;
mod network;
mod open;
mod playback;
mod saved;
mod server;
mod user;

pub use appearance::AppTheme;
pub use artwork::{Artwork, AspectRatio, ImageType};
pub use capabilities::{Capabilities, Feature, Reachability, ServerStatus};
pub use engine::{Container, EngineCapabilities, VideoCodecSupport};
pub use identity::{
    ItemId, ItemRef, MediaKind, ServerId, ServerKey, ServerRef, ServiceType, UserId,
};
pub use network::{Connectivity, InterfaceKind, LocalInterface, NetworkFacts, NetworkKey};
pub use playback::{
    AudioFacts, Codec, EngineErrorKind, MediaSource, MediaStream, PlaybackPlan,
    PlaybackPreferences, PlaybackProblem, Route, RouteLabel, RouteReason, Segment, SegmentKind,
    StreamAction, StreamKind, StreamProtocol, SubtitleChoice, SubtitleDelivery, SubtitleMode,
    TrackChoice, TranscodingFacts, VideoFacts, VideoRange, VideoRangeType,
};
pub use saved::{AuthHeader, SavedServer, SavedUser};
pub use server::{AddressId, GatewayKind, PublicServerInfo, ServerAddress};
pub use user::{Permission, User};

/// Version of the core, from the workspace manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comes_from_the_workspace() {
        assert_eq!(version(), "0.1.0");
    }
}
