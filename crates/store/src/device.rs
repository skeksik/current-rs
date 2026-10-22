//! The device identity: one random id per install, sent to servers as Jellyfin's `DeviceId` (ADR 0013).

use serde::{Deserialize, Serialize};

use crate::StoreError;

/// Random bytes in a new id: 128 bits.
const ID_BYTES: usize = 16;
/// Longest id accepted from the settings file.
const MAX_LEN: usize = 64;

/// This install's id. Made once from the OS random number generator and kept in the settings file; never derived from hardware.
///
/// Jellyfin keeps one device record and one access token per device id and user, and signing in again on the same id replaces
/// the old token. So the id must stay the same for the life of the install and must differ between installs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeviceId(String);

impl DeviceId {
    /// A new random id: 32 lowercase hex digits.
    pub fn generate() -> Result<Self, StoreError> {
        let mut bytes = [0u8; ID_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| StoreError::Random)?;
        Ok(Self(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    /// An id read back from storage: 1 to 64 ASCII letters, digits or hyphens, so it fits unescaped in a quoted header parameter.
    /// Ids made by an earlier version in another format still pass.
    pub fn parse(id: &str) -> Option<Self> {
        let valid = !id.is_empty()
            && id.len() <= MAX_LEN
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        valid.then(|| Self(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for DeviceId {
    type Error = String;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::parse(&id).ok_or_else(|| "not a valid device id".to_owned())
    }
}

impl From<DeviceId> for String {
    fn from(id: DeviceId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_id_is_32_lowercase_hex_digits() {
        let id = DeviceId::generate().unwrap();
        assert_eq!(id.as_str().len(), 32);
        assert!(
            id.as_str()
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );
    }

    #[test]
    fn new_ids_differ() {
        assert_ne!(DeviceId::generate().unwrap(), DeviceId::generate().unwrap());
    }

    #[test]
    fn parse_accepts_header_safe_ids_only() {
        assert!(DeviceId::parse("0123abcd").is_some());
        assert!(DeviceId::parse("A-1").is_some());
        assert!(DeviceId::parse("").is_none());
        assert!(DeviceId::parse("a b").is_none());
        assert!(DeviceId::parse("a\"b").is_none());
        assert!(DeviceId::parse(&"a".repeat(65)).is_none());
    }
}
