//! What the engine reports: mpv's events and observed properties as safe values (ADR 0019 "Event thread", "Observed properties").

use crate::error::MpvError;
use crate::node::Node;

/// The observed properties, by their reply id (index + 1). URL-valued properties (`path`, `stream-open-filename`, `playlist`,
/// `filename`) are never observed or read (ADR 0019, redaction).
pub(crate) const OBSERVED: [&str; 16] = [
    "time-pos",
    "pause",
    "paused-for-cache",
    "seeking",
    "duration",
    "seekable",
    "eof-reached",
    "track-list",
    "aid",
    "sid",
    "demuxer-cache-time",
    "video-params",
    "hwdec-current",
    "volume",
    "mute",
    "display-swapchain",
];

/// The reply id of an observed property.
#[cfg(test)]
fn observe_id(name: &str) -> Option<u64> {
    OBSERVED
        .iter()
        .position(|p| *p == name)
        .map(|i| i as u64 + 1)
}

/// One event from the engine, in mpv's order.
#[derive(Debug)]
pub enum EngineEvent {
    /// A file started loading.
    StartFile,
    /// The file is open and its tracks are known.
    FileLoaded,
    /// Playback (re)started after loading or a seek: the first frame is out.
    PlaybackRestart,
    /// A seek started.
    Seek,
    /// A file ended.
    EndFile {
        reason: EndReason,
        error: Option<MpvError>,
    },
    /// Nothing is loaded.
    Idle,
    Property(PropertyChange),
    /// A new swap chain (Windows composition mode). The engine already holds a reference for the receiver.
    SwapChain(crate::swapchain::SwapChain),
    /// An asynchronous command or property change failed. `what` names it, never its value.
    CommandFailed {
        what: &'static str,
        error: MpvError,
    },
    /// A log line from mpv, already redacted.
    Log(LogLine),
    /// mpv's event queue overflowed and events were lost: the controller should re-read its state.
    Overflow,
    /// mpv quit on its own (not asked by the engine): nothing more will play. Drop the engine; [`EngineEvent::Shutdown`] follows then.
    Terminated,
    /// The engine stopped; no events follow.
    Shutdown,
}

/// Why a file ended (`mpv_end_file_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    Stop,
    Quit,
    Error,
    Redirect,
    /// A reason newer than client API 2.5.
    Other(i32),
}

impl EndReason {
    pub(crate) fn from_raw(raw: i32) -> EndReason {
        match raw {
            0 => EndReason::Eof,
            2 => EndReason::Stop,
            3 => EndReason::Quit,
            4 => EndReason::Error,
            5 => EndReason::Redirect,
            other => EndReason::Other(other),
        }
    }
}

/// A redacted line from mpv's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// mpv's level name: `fatal`, `error`, `warn`, `info`, `v`, `debug`, `trace`.
    pub level: String,
    /// The module (`ffmpeg/demuxer`, `cplayer`, ...).
    pub prefix: String,
    /// The text without its trailing newline.
    pub text: String,
}

/// A change of one observed property. `None` means mpv has no value (nothing loaded, or not known yet).
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyChange {
    TimePos(Option<f64>),
    Pause(bool),
    PausedForCache(bool),
    Seeking(bool),
    Duration(Option<f64>),
    Seekable(bool),
    EofReached(bool),
    TrackList(Vec<Track>),
    Aid(Option<i64>),
    Sid(Option<i64>),
    DemuxerCacheTime(Option<f64>),
    VideoParams(Option<VideoParams>),
    HwdecCurrent(Option<String>),
    Volume(f64),
    Mute(bool),
}

/// A track as mpv lists it (`track-list`). Never its `external-filename`, which can be a URL.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub id: i64,
    /// `video`, `audio` or `sub`.
    pub kind: String,
    /// FFmpeg's stream index, for joining with the server's stream index (ADR 0019 "Tracks").
    pub ff_index: Option<i64>,
    pub lang: Option<String>,
    pub title: Option<String>,
    pub codec: Option<String>,
    pub default: bool,
    pub forced: bool,
    pub external: bool,
    pub selected: bool,
}

/// What `video-params` says about the decoded video.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VideoParams {
    pub width: i64,
    pub height: i64,
    pub pixel_format: Option<String>,
    pub primaries: Option<String>,
    pub gamma: Option<String>,
}

/// What the event thread makes of a property change: a value, a swap chain pointer to take over, or nothing (unknown id, or a value of
/// an unexpected shape).
#[derive(Debug, PartialEq)]
pub(crate) enum Observed {
    Change(PropertyChange),
    /// `display-swapchain`: the address of mpv's `IDXGISwapChain`, 0 when there is none.
    SwapChain(u64),
    Ignore,
}

pub(crate) fn observed(id: u64, value: &Node) -> Observed {
    let Some(name) = id.checked_sub(1).and_then(|i| OBSERVED.get(i as usize)) else {
        return Observed::Ignore;
    };
    let flag = |f: fn(bool) -> PropertyChange| value.as_bool().map(f);
    let change = match *name {
        "time-pos" => Some(PropertyChange::TimePos(value.as_f64())),
        "pause" => flag(PropertyChange::Pause),
        "paused-for-cache" => flag(PropertyChange::PausedForCache),
        "seeking" => flag(PropertyChange::Seeking),
        "duration" => Some(PropertyChange::Duration(value.as_f64())),
        "seekable" => flag(PropertyChange::Seekable),
        "eof-reached" => flag(PropertyChange::EofReached),
        "track-list" => Some(PropertyChange::TrackList(
            value
                .as_array()
                .unwrap_or_default()
                .iter()
                .map(track)
                .collect(),
        )),
        // `aid`/`sid` are a track id, or `no`/`auto` as a string.
        "aid" => Some(PropertyChange::Aid(value.as_i64())),
        "sid" => Some(PropertyChange::Sid(value.as_i64())),
        "demuxer-cache-time" => Some(PropertyChange::DemuxerCacheTime(value.as_f64())),
        "video-params" => Some(PropertyChange::VideoParams(video_params(value))),
        "hwdec-current" => Some(PropertyChange::HwdecCurrent(
            value.as_str().map(str::to_owned),
        )),
        "volume" => value.as_f64().map(PropertyChange::Volume),
        "mute" => flag(PropertyChange::Mute),
        "display-swapchain" => {
            // An address; a negative or missing value means none.
            return Observed::SwapChain(
                value
                    .as_i64()
                    .and_then(|a| u64::try_from(a).ok())
                    .unwrap_or(0),
            );
        }
        _ => None,
    };
    change.map_or(Observed::Ignore, Observed::Change)
}

fn text(node: &Node, key: &str) -> Option<String> {
    node.get(key).and_then(Node::as_str).map(str::to_owned)
}

fn flag(node: &Node, key: &str) -> bool {
    node.get(key).and_then(Node::as_bool).unwrap_or(false)
}

fn track(node: &Node) -> Track {
    Track {
        id: node.get("id").and_then(Node::as_i64).unwrap_or(0),
        kind: text(node, "type").unwrap_or_default(),
        ff_index: node.get("ff-index").and_then(Node::as_i64),
        lang: text(node, "lang"),
        title: text(node, "title"),
        codec: text(node, "codec"),
        default: flag(node, "default"),
        forced: flag(node, "forced"),
        external: flag(node, "external"),
        selected: flag(node, "selected"),
    }
}

fn video_params(node: &Node) -> Option<VideoParams> {
    let width = node.get("w").and_then(Node::as_i64)?;
    let height = node.get("h").and_then(Node::as_i64)?;
    Some(VideoParams {
        width,
        height,
        pixel_format: text(node, "pixelformat"),
        primaries: text(node, "primaries"),
        gamma: text(node, "gamma"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> u64 {
        observe_id(name).unwrap()
    }

    #[test]
    fn never_observes_url_properties() {
        for url in [
            "path",
            "stream-open-filename",
            "playlist",
            "filename",
            "stream-path",
        ] {
            assert!(observe_id(url).is_none(), "{url}");
        }
        assert_eq!(observe_id("time-pos"), Some(1));
        assert_eq!(observe_id("display-swapchain"), Some(16));
    }

    #[test]
    fn maps_scalar_properties() {
        assert_eq!(
            observed(id("time-pos"), &Node::Double(1.5)),
            Observed::Change(PropertyChange::TimePos(Some(1.5)))
        );
        assert_eq!(
            observed(id("time-pos"), &Node::None),
            Observed::Change(PropertyChange::TimePos(None))
        );
        assert_eq!(
            observed(id("pause"), &Node::Flag(true)),
            Observed::Change(PropertyChange::Pause(true))
        );
        assert_eq!(observed(id("pause"), &Node::None), Observed::Ignore);
        assert_eq!(
            observed(id("aid"), &Node::Int(2)),
            Observed::Change(PropertyChange::Aid(Some(2)))
        );
        assert_eq!(
            observed(id("sid"), &Node::String("no".into())),
            Observed::Change(PropertyChange::Sid(None))
        );
        assert_eq!(
            observed(id("volume"), &Node::Double(80.0)),
            Observed::Change(PropertyChange::Volume(80.0))
        );
        assert_eq!(
            observed(id("display-swapchain"), &Node::Int(0x1234)),
            Observed::SwapChain(0x1234)
        );
        assert_eq!(
            observed(id("display-swapchain"), &Node::None),
            Observed::SwapChain(0)
        );
        assert_eq!(observed(0, &Node::None), Observed::Ignore);
        assert_eq!(observed(99, &Node::None), Observed::Ignore);
    }

    #[test]
    fn maps_tracks_and_video_params() {
        let list = Node::Array(vec![Node::Map(vec![
            ("id".into(), Node::Int(1)),
            ("type".into(), Node::String("audio".into())),
            ("ff-index".into(), Node::Int(1)),
            ("lang".into(), Node::String("eng".into())),
            ("codec".into(), Node::String("eac3".into())),
            ("default".into(), Node::Flag(true)),
            ("selected".into(), Node::Flag(true)),
            (
                "external-filename".into(),
                Node::String("https://media.example.com/s.srt".into()),
            ),
        ])]);
        let Observed::Change(PropertyChange::TrackList(tracks)) = observed(id("track-list"), &list)
        else {
            panic!()
        };
        assert_eq!(
            tracks,
            vec![Track {
                id: 1,
                kind: "audio".into(),
                ff_index: Some(1),
                lang: Some("eng".into()),
                codec: Some("eac3".into()),
                default: true,
                selected: true,
                ..Track::default()
            }]
        );
        let params = Node::Map(vec![
            ("w".into(), Node::Int(3840)),
            ("h".into(), Node::Int(2160)),
            ("pixelformat".into(), Node::String("p010".into())),
            ("primaries".into(), Node::String("bt.2020".into())),
            ("gamma".into(), Node::String("pq".into())),
        ]);
        let Observed::Change(PropertyChange::VideoParams(Some(p))) =
            observed(id("video-params"), &params)
        else {
            panic!()
        };
        assert_eq!(
            (p.width, p.height, p.gamma.as_deref()),
            (3840, 2160, Some("pq"))
        );
        assert_eq!(
            observed(id("video-params"), &Node::None),
            Observed::Change(PropertyChange::VideoParams(None))
        );
    }

    #[test]
    fn end_reasons() {
        assert_eq!(EndReason::from_raw(0), EndReason::Eof);
        assert_eq!(EndReason::from_raw(4), EndReason::Error);
        assert_eq!(EndReason::from_raw(42), EndReason::Other(42));
    }
}
