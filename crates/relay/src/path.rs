//! Which paths a route lets through: those inside its prefixes (`/Videos/{itemId}/`, `/Audio/{itemId}/`), with nothing that could
//! climb out of them once the server or a URL parser normalizes the path.

use crate::RelayError;

/// A route's path prefixes, checked when the route is created.
#[derive(Clone, Debug)]
pub(crate) struct Prefixes(Vec<String>);

impl Prefixes {
    /// Each prefix must be an absolute path that ends with `/`, in printable ASCII, with no dot segment, query or fragment.
    pub(crate) fn new(prefixes: Vec<String>) -> Result<Self, RelayError> {
        let valid = |prefix: &String| {
            prefix.len() > 2
                && prefix.starts_with('/')
                && prefix.ends_with('/')
                && prefix
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && b != b'?' && b != b'#')
                && is_clean(prefix)
        };
        if prefixes.is_empty() || !prefixes.iter().all(valid) {
            return Err(RelayError::InvalidPrefix);
        }
        Ok(Self(prefixes))
    }

    /// Whether `path` (percent-encoded, relative to the server's address) is inside one of the prefixes and clean. Prefixes match
    /// without regard to ASCII case: Jellyfin's routes do not care (its `TranscodingUrl` says `/videos/`, ADR 0019's notes).
    pub(crate) fn allow(&self, path: &str) -> bool {
        is_clean(path)
            && self.0.iter().any(|prefix| {
                path.len() > prefix.len()
                    && path.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
            })
    }
}

/// No `.` or `..` segment (also percent-encoded, which WHATWG URL parsers treat the same), no encoded `/` or `\`, no `\`, no
/// control character or space.
fn is_clean(path: &str) -> bool {
    if path
        .bytes()
        .any(|b| b == b'\\' || b.is_ascii_control() || b == b' ' || !b.is_ascii())
    {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2f") || lower.contains("%5c") {
        return false;
    }
    lower.split('/').all(|segment| {
        let segment = segment.replace("%2e", ".");
        segment != "." && segment != ".."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes() -> Prefixes {
        Prefixes::new(vec!["/Videos/abc/".into(), "/Audio/abc/".into()]).unwrap()
    }

    #[test]
    fn paths_inside_a_prefix_pass_in_any_case() {
        let prefixes = prefixes();
        for path in [
            "/Videos/abc/stream",
            "/videos/ABC/master.m3u8",
            "/Audio/abc/universal",
            "/Videos/abc/hls1/main/0.mp4",
            "/Videos/abc/x%20y",
        ] {
            assert!(prefixes.allow(path), "{path}");
        }
    }

    #[test]
    fn paths_outside_or_climbing_out_are_refused() {
        let prefixes = prefixes();
        for path in [
            "/Videos/abc/",
            "/Videos/abcd/stream",
            "/Videos/xyz/stream",
            "/Users/abc/x",
            "/Videos/abc/../../Users",
            "/Videos/abc/%2e%2e/%2E%2E/Users",
            "/Videos/abc/.%2e/x",
            "/Videos/abc/./x",
            "/Videos/abc%2f..%2fx",
            "/Videos/abc/..%5cx",
            "/Videos/abc/a\\b",
            "/Videos/abc/a b",
            "Videos/abc/x",
        ] {
            assert!(!prefixes.allow(path), "{path}");
        }
    }

    #[test]
    fn prefixes_must_be_clean_absolute_directories() {
        for bad in [
            vec![],
            vec!["/".to_string()],
            vec!["/Videos/abc".to_string()],
            vec!["Videos/abc/".to_string()],
            vec!["/Videos/../".to_string()],
            vec!["/Videos/a?b/".to_string()],
            vec!["/Videos/a b/".to_string()],
        ] {
            assert!(Prefixes::new(bad.clone()).is_err(), "{bad:?}");
        }
    }
}
