//! An owned copy of mpv's `mpv_node` tree, so that everything past the `sys` boundary is safe Rust and testable without libmpv.

/// A value as mpv's client API describes it (`mpv_format`), owned.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    None,
    String(String),
    Flag(bool),
    Int(i64),
    Double(f64),
    Array(Vec<Node>),
    Map(Vec<(String, Node)>),
    Bytes(Vec<u8>),
}

impl Node {
    /// The value under `key` when this is a map.
    pub(crate) fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Node::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Node::Flag(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn as_i64(&self) -> Option<i64> {
        match self {
            Node::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// A number, from either an `INT64` or a `DOUBLE` node.
    pub(crate) fn as_f64(&self) -> Option<f64> {
        match self {
            Node::Double(d) => Some(*d),
            // Exact for every value mpv reports here (track ids, sizes, seconds).
            Node::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&[Node]> {
        match self {
            Node::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Shorthand for building a map node from string pairs.
    pub(crate) fn string_map<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Node {
        Node::Map(
            pairs
                .into_iter()
                .map(|(k, v)| (k.into(), Node::String(v.into())))
                .collect(),
        )
    }
}
