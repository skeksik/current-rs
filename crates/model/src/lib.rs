//! Domain models shared by every Current client. Pure Rust: no UI, no I/O.
//!
//! The types are plain records and enums with owned fields (no generics or lifetimes in their public shape) so that they can cross the
//! UniFFI bridge later (ADR 0002, ADR 0005).

mod artwork;
mod capabilities;
mod identity;
mod open;
mod server;
mod user;

pub use artwork::{Artwork, AspectRatio, ImageType};
pub use capabilities::{Capabilities, Feature, Reachability, ServerStatus};
pub use identity::{
    ItemId, ItemRef, MediaKind, ServerId, ServerKey, ServerRef, ServiceType, UserId,
};
pub use server::{PublicServerInfo, ServerAddress};
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
