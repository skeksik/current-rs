//! What the linked libmpv can play, read once after `mpv_initialize` from `decoder-list` and `demuxer-lavf-list` (ADR 0019
//! "Capabilities"), so a different build reports what it really has.

use std::collections::BTreeSet;

use crate::node::Node;

/// mpv's own names: `decoder-list`'s `codec` entries (audio and video decoders; mpv fills it from `vd_lavc.c` and `ad_lavc.c`, so no
/// subtitle decoders) and `demuxer-lavf-list`'s FFmpeg demuxer names (`matroska,webm`, `mov,mp4,m4a,3gp,3g2,mj2`, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildCapabilities {
    pub decoders: BTreeSet<String>,
    pub demuxers: BTreeSet<String>,
}

impl BuildCapabilities {
    /// From the two property values. Entries of an unexpected shape are skipped.
    pub(crate) fn from_nodes(decoder_list: &Node, demuxer_list: &Node) -> BuildCapabilities {
        let decoders = decoder_list
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry.get("codec").and_then(Node::as_str))
            .map(str::to_owned)
            .collect();
        let demuxers = demuxer_list
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(Node::as_str)
            .map(str::to_owned)
            .collect();
        BuildCapabilities { decoders, demuxers }
    }

    pub fn has_decoder(&self, codec: &str) -> bool {
        self.decoders.contains(codec)
    }

    pub fn has_demuxer(&self, name: &str) -> bool {
        self.demuxers.contains(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoder(codec: &str, driver: &str) -> Node {
        Node::Map(vec![
            ("codec".into(), Node::String(codec.into())),
            ("driver".into(), Node::String(driver.into())),
            ("description".into(), Node::String(String::new())),
        ])
    }

    #[test]
    fn reads_codecs_and_demuxers() {
        let decoders = Node::Array(vec![
            decoder("h264", "h264"),
            decoder("hevc", "hevc"),
            decoder("av1", "libdav1d"),
            decoder("av1", "av1"),
            Node::String("junk".into()),
        ]);
        let demuxers = Node::Array(vec![
            Node::String("matroska,webm".into()),
            Node::String("hls".into()),
            Node::Int(1),
        ]);
        let caps = BuildCapabilities::from_nodes(&decoders, &demuxers);
        assert_eq!(
            caps.decoders.iter().map(String::as_str).collect::<Vec<_>>(),
            ["av1", "h264", "hevc"]
        );
        assert!(caps.has_demuxer("matroska,webm"));
        assert!(caps.has_demuxer("hls"));
        assert!(!caps.has_decoder("vc1"));
        assert_eq!(
            BuildCapabilities::from_nodes(&Node::None, &Node::None),
            BuildCapabilities::default()
        );
    }
}
