//! Domain models shared by every Current client. Pure Rust: no UI, no I/O.

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
