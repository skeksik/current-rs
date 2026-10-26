//! Route tokens: 128 random bits from the OS (`getrandom`), base64url without padding, compared in constant time (ADR 0020, "Routes
//! and who may use it").

use crate::RelayError;

/// The random bytes in a token.
const TOKEN_BYTES: usize = 16;

/// The URL-safe base64 alphabet (RFC 4648 §5).
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// A route's token: the first path segment of every URL the player gets for the route. Debug never shows it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Token(String);

impl Token {
    pub(crate) fn random() -> Result<Self, RelayError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| RelayError::Random)?;
        Ok(Self(base64url(&bytes)))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `candidate` is this token, in time that depends only on the lengths: every byte is compared.
    pub(crate) fn matches(&self, candidate: &str) -> bool {
        let (a, b) = (self.0.as_bytes(), candidate.as_bytes());
        if a.len() != b.len() {
            return false;
        }
        let difference = a
            .iter()
            .zip(b)
            .fold(0u8, |acc, (x, y)| std::hint::black_box(acc | (x ^ y)));
        difference == 0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token([redacted])")
    }
}

/// `bytes` in base64url without padding (RFC 4648 §5).
fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        // A group of k bytes gives k + 1 characters.
        for i in 0..=group.len() {
            let index = (n >> (18 - 6 * i)) & 0x3f;
            out.push(char::from(ALPHABET[index as usize]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_rfc_4648_test_vectors() {
        // RFC 4648 §10, without padding, and the two characters only base64url has.
        for (input, output) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url(input.as_bytes()), output);
        }
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn a_token_is_22_url_safe_characters_and_new_each_time() {
        let a = Token::random().unwrap();
        let b = Token::random().unwrap();
        assert_eq!(a.as_str().len(), 22);
        assert!(
            a.as_str()
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_ne!(a, b);
        assert!(a.matches(a.as_str()));
        assert!(!a.matches(b.as_str()));
        assert!(!a.matches(&a.as_str()[1..]));
        assert!(!a.matches(""));
        assert_eq!(format!("{a:?}"), "Token([redacted])");
    }
}
