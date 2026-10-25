//! 128-bit FNV-1a, the cache's file-name hash (ADR 0018, open choice 7). Not cryptographic, which the cache does not need: its
//! inputs are server keys, item ids and tags, not secrets, and a 128-bit collision among one cache's files is negligible.
//!
//! The algorithm and constants are from the IETF's FNV document (draft-eastlake-fnv-35, 6 June 2025, published as RFC 9923; read
//! 2026-10-25), section 2: "hash = offset_basis; for each octet_of_data to be hashed: hash = hash xor octet_of_data; hash = hash *
//! FNV_Prime mod 2**HashSize", and section 5: "128-bit FNV_Prime = 2**88 + 2**8 + 0x3B", offset_basis "0x6C62272E 07BB0142
//! 62B82175 6295C58D".

const OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const PRIME: u128 = (1 << 88) + (1 << 8) + 0x3b;

/// A running 128-bit FNV-1a hash.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fnv128(u128);

impl Fnv128 {
    pub(crate) fn new() -> Self {
        Self(OFFSET_BASIS)
    }

    pub(crate) fn bytes(mut self, data: &[u8]) -> Self {
        for &byte in data {
            self.0 ^= u128::from(byte);
            self.0 = self.0.wrapping_mul(PRIME);
        }
        self
    }

    /// A field: its length first, so that ("ab", "c") and ("a", "bc") hash apart.
    pub(crate) fn field(self, data: &[u8]) -> Self {
        self.bytes(&(data.len() as u64).to_le_bytes()).bytes(data)
    }

    /// The hash as 32 lowercase hex digits: a file or folder name.
    pub(crate) fn hex(self) -> String {
        format!("{:032x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_published_test_vectors() {
        // The expected values of `FNV128svalues` in the document's test program (`Test128()`), for its test strings
        // "", "a", "foobar" and "Hello!\x01\xFF\xED", read 2026-10-25.
        for (input, expected) in [
            (&b""[..], "6c62272e07bb014262b821756295c58d"),
            (b"a", "d228cb696f1a8caf78912b704e4a8964"),
            (b"foobar", "343e1662793c64bf6f0d3597ba446f18"),
            (b"Hello!\x01\xFF\xED", "74202c600b051c165b1acafed10d1419"),
        ] {
            assert_eq!(Fnv128::new().bytes(input).hex(), expected);
        }
    }

    #[test]
    fn fields_are_delimited() {
        let a = Fnv128::new().field(b"ab").field(b"c").hex();
        let b = Fnv128::new().field(b"a").field(b"bc").hex();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(
            a.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
    }
}
